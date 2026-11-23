---
title: forge system
description: Show the Forge server's version and update status, ask the release feed for a newer version, and install a server update.
sidebar:
  order: 23
---

`forge system` updates the Forge server itself, the same way as **Update now**
in the console. For what an update does step by step, see
[Update Forge](/docs/guides/update-forge/). To update
the agents on your nodes afterwards, see [Update agents](/docs/guides/update-agents/).
To update the `forge` CLI, use [`forge update`](/docs/reference/cli/update/). Every
command takes the [global flags](/docs/reference/cli/#global-flags).

## Commands

| Command | What it does |
| --- | --- |
| [`forge system update`](#forge-system-update) | Shows the server version and whether an update is available |
| [`forge system check`](#forge-system-check) | Asks the release feed for a newer server version now |
| [`forge system releases`](#forge-system-releases) | Lists the releases you can upgrade to, newest first |
| [`forge system apply`](#forge-system-apply) | Installs a server update; the server backs up its database and restarts |

### forge system update

Shows the server's version, the latest release the server knows of, and the
state of an update in progress. Any signed-in user can run it; `can_update`
tells you whether you may install the update.

The server asks the release feed a minute after it starts and then every six
hours, unless `[update] enabled = false` is set in its configuration. Until
the first check, `latest` and `checked_at` are `null`.

**Synopsis**

```text
forge system update
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
# Is a server update available?
forge system update
# Only the yes/no answer, for a script
forge -o json system update | jq '.result.available'
```

**Output**

The table output prints the status as a key/value list. With `-o json`, the same
object is in `result`:

```json
$ forge -o json system update
{
  "result": {
    "available": false,
    "can_update": true,
    "checked_at": 1794589923096,
    "current": "0.1.1-dev.20261113.214755",
    "enabled": true,
    "latest": {
      "assets": {
        "SHA256SUMS": "http://192.0.2.1:8000/mr-aybee/forge/releases/download/v0.1.1-dev.20261113.214755/SHA256SUMS",
        "SHA256SUMS.sig": "http://192.0.2.1:8000/mr-aybee/forge/releases/download/v0.1.1-dev.20261113.214755/SHA256SUMS.sig",
        "forge-server-x86_64-unknown-linux-musl": "http://192.0.2.1:8000/mr-aybee/forge/releases/download/v0.1.1-dev.20261113.214755/forge-server-x86_64-unknown-linux-musl",
        …
      },
      "notes": "Download crash fix, compress formats, console consistency, forge update, typed schemas",
      "published_at": "2026-11-13T17:10:50Z",
      "url": "",
      "version": "0.1.1-dev.20261113.214755"
    },
    "message": null,
    "state": "idle",
    "target": "x86_64-unknown-linux-musl"
  },
  "schema": "forge.cli/v1"
}
```

| Field | Description |
| --- | --- |
| `current` | The running server's version |
| `latest` | The newest release from the feed: `version`, `notes`, `published_at`, release page `url` and download `assets`. `null` before the first check |
| `available` | `true` when `latest` is newer than `current` |
| `checked_at` | When the server last asked the feed, in epoch milliseconds |
| `state` | `idle`, `downloading`, `verifying`, `installing`, `restarting` or `failed` |
| `message` | What the update is doing, or why it failed |
| `enabled` | Whether the server checks the feed on its own (`[update] enabled`) |
| `can_update` | Whether you hold `system.update` and may install it |
| `target` | The server's build target, which picks the release files |

Exit codes:

- `0`: success.
- `3`: not signed in.

**See also**: [`forge system check`](#forge-system-check),
[`forge system apply`](#forge-system-apply).

### forge system check

Asks the release feed for a newer server version now, instead of waiting for
the next scheduled check. Needs `system.update` on the platform (role
`platform_admin`) and an MFA-verified session (step-up).

**Synopsis**

```text
forge system check
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
# Check the feed now
forge system check
# Check, then print the newest version
forge -o json system check | jq -r '.result.latest.version'
```

**Output**

The same object as [`forge system update`](#forge-system-update), with
`can_update: true` and a fresh `checked_at`. Not captured separately.

Exit codes:

- `0`: the feed answered.
- `2`: the server cannot reach the release feed.
- `3`: not signed in.
- `4`: you lack `system.update`.
- `5`: the action needs an MFA-verified session (`step_up_required`). In a
  terminal the CLI asks for a one-time code and retries; with
  `--non-interactive` or `FORGE_TOKEN`, run `forge auth step-up` first.

**See also**: [`forge system apply`](#forge-system-apply),
[`forge auth`](/docs/reference/cli/auth/).

### forge system releases

Lists the releases the server can upgrade to, newest first, with the running
version marked `CURRENT`. Older versions are never listed: Forge only
upgrades. Drafts and prereleases are left out, unless the server itself runs
a prerelease (such as a `-dev.` lab build). Needs `view` on the platform.

**Synopsis**

```text
forge system releases
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
# Which versions can I install?
forge system releases
# The newest version, for a script
forge -o json system releases | jq -r '.result[0].version'
```

**Output**

Shape from the server's schema; not captured on a live system.

```text
$ forge system releases
VERSION  PUBLISHED             CURRENT
0.3.0    2026-11-14T09:00:00Z  false
0.2.1    2026-11-02T10:30:00Z  false
0.2.0    2026-10-20T08:15:00Z  true
```

Exit codes:

- `0`: success.
- `2`: the server cannot reach the release feed.
- `3`: not signed in.

**See also**: [`forge system apply`](#forge-system-apply).

### forge system apply

Installs a server update. The server downloads the release, checks its
Ed25519 signature and checksums, test-runs the new version, backs up a SQLite
database, installs the new binaries and restarts. Needs `system.update` on
the platform (role `platform_admin`) and an MFA-verified session (step-up).
The command asks for confirmation first. Forge audits the update as
`system.update`, with `from`, `to` and `agents`.

You can install any listed release that is newer than the running one, not
only the latest. Forge never installs an older or the same version. With
`--agents rollout` (the default), the new server starts a canary rollout of
every outdated online Linux agent to its own version once it serves and the
agents have reconnected. The rollout runs as you, so you also need
`server.command` on the nodes. Windows agents need a reinstall, as described
in [Update agents](/docs/guides/update-agents/).

The command returns as soon as the server has started the update. Run
`forge system update` to follow `state` until the server answers with the new
version.

**Synopsis**

```text
forge system apply [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--version <VERSION>` | string, version | the newest release from `forge system releases` | no | The version to install. It must be a listed release newer than the running one. `--to` is an alias | `--version 0.2.1` |
| `--agents <AGENTS>` | `rollout` or `none` | `rollout` | no | After the restart, update the agents to the new version in canary batches (`rollout`), or leave them (`none`) | `--agents none` |
| `--batch <BATCH>` | integer, 1-100 | `1` | no | Nodes per batch of the agent rollout. The first batch is the canary | `--batch 5` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` | `--yes` |

**Examples**

```sh
# Install the latest version the server offers, after a prompt
forge system apply
# Install a version you reviewed, without a prompt
forge auth step-up
forge system apply --version <version> --yes
# Install without touching the agents
forge system apply --version <version> --agents none --yes
# Follow the update until the server restarts
forge -o json system update | jq -r '.result.state + " " + (.result.message // "")'
```

**Output**

Shape from the server's schema; not captured on a live system.

```json
$ forge -o json system apply --version <version> --yes
{
  "result": { "agents": "rollout", "state": "downloading", "version": "<version>" },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: the update started.
- `1`: you answered anything but `yes` at the prompt.
- `2`: `--yes` is missing without a terminal.
- `3`: not signed in.
- `4`: you lack `system.update`.
- `5`: the action needs an MFA-verified session (`step_up_required`). In a
  terminal the CLI asks for a one-time code and retries; with
  `--non-interactive`, run `forge auth step-up` first; with `FORGE_TOKEN`,
  use an elevated token.
- `7`: no update is available (`up_to_date`), the version is not newer than
  the running one, the feed does not offer it, or an update is already in
  progress.

:::caution
The server restarts, so the console, the API and the CLI are unavailable for
a short time. Nodes keep running. Forge backs up only a SQLite database, to
`<data_dir>/backups/forge-<old-version>-<time>.db`. With PostgreSQL, take
your own backup first.
:::

**See also**: [`forge system update`](#forge-system-update),
[Upgrade notes](/docs/reference/upgrade-notes/),
[Update agents](/docs/guides/update-agents/).
