---
title: forge update
description: Replace the forge CLI with the latest signed release after checking its signature, checksum and that it starts, on Linux and Windows.
sidebar:
  order: 28
---

`forge update` replaces the running `forge` binary with the latest signed
release. It works the same on Linux and Windows, and needs no sign-in. Releases
include no macOS binary, so a CLI built from source on macOS cannot update
itself this way.
To update the Forge server, use [`forge system`](/docs/reference/cli/system/)
instead. It takes the [global flags](/docs/reference/cli/#global-flags).

## Commands

| Command | What it does |
| --- | --- |
| [`forge update`](#forge-update) | Updates this CLI, or only reports whether an update exists |

### forge update

Updates the CLI. Each step must pass before anything changes:

1. Read the release feed and compare its version with this build's.
2. Download `SHA256SUMS` and `SHA256SUMS.sig` and check the Ed25519
   signature with the release key compiled into `forge`.
3. Download `forge-cli-<target>` for this platform and check its SHA-256
   against the signed sums.
4. Write it next to the running binary and run it once: `--version` must
   print the new version.
5. Swap it in. The old binary stays as `forge.prev` (`forge.exe.prev` on
   Windows) until the next update.

When any step fails, the installed binary is unchanged. The feed and every
download must use HTTPS; plain HTTP is allowed only to a loopback address. If
`forge` is a symbolic link, the update replaces the file it points to.

The update needs write access to the folder that holds `forge`. If you
installed it with `sudo` into `/usr/local/bin`, run `sudo forge update`.

**Synopsis**

```text
forge update [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--check` | flag | off | no | Only report `current`, `latest`, `available`, `url` and `feed`; change nothing | `--check` |
| `--allow-downgrade` | flag | off | no | Install the feed's release even when it is older than this one | `--allow-downgrade` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` | `--yes` |

**Environment variables**

| Variable | Type | Default | Description | Example |
| --- | --- | --- | --- | --- |
| `FORGE_UPDATE_FEED` | URL | the public release feed, `https://api.github.com/repos/orbyntech/forge/releases/latest` | A GitHub-style "latest release" document, for a mirror. HTTPS only; plain HTTP only to a loopback address | `https://<mirror-host>/releases/latest` |
| `FORGE_UPDATE_PUBLIC_KEY` | string, base64url | the Forge release key | The Ed25519 public key that signs the mirror's `SHA256SUMS` | `<base64url-public-key>` |

:::caution
Setting `FORGE_UPDATE_PUBLIC_KEY` changes whom `forge` trusts for its own
code. Set it only for a mirror you sign yourself.
:::

**Examples**

```sh
# Is a newer CLI out?
forge update --check
# Update after a prompt
forge update
# Update from a script, for a CLI installed into /usr/local/bin
sudo forge update --yes --non-interactive
# Update from a mirror you sign yourself
FORGE_UPDATE_FEED=https://<mirror-host>/releases/latest FORGE_UPDATE_PUBLIC_KEY=<base64url-public-key> forge update
```

**Output**

`--check`, captured against a test release; the feed is the default one:

```text
$ forge update --check
{
  "available": true,
  "current": "0.1.0",
  "feed": "https://api.github.com/repos/orbyntech/forge/releases/latest",
  "latest": "0.1.1-dev.20261113.214755",
  "url": "https://github.com/orbyntech/forge/releases/tag/v0.1.1-dev.20261113.214755"
}
```

```text
$ forge -o json update --check
{"result":{"available":true,"current":"0.1.0","feed":"https://api.github.com/repos/orbyntech/forge/releases/latest","latest":"0.1.1-dev.20261113.214755","url":"https://github.com/orbyntech/forge/releases/tag/v0.1.1-dev.20261113.214755"},"schema":"forge.cli/v1"}
```

| Field | Description |
| --- | --- |
| `current` | This CLI's version |
| `latest` | The feed's version |
| `available` | `true` when `latest` is newer than `current` |
| `url` | The release page |
| `feed` | The feed that was read |

An update prints the following. Shape from the CLI's source; not captured on
a live system.

```json
$ forge -o json update --yes
{
  "result": {
    "previous": "<old-version>",
    "current": "<new-version>",
    "updated": true,
    "path": "/usr/local/bin/forge",
    "backup": "/usr/local/bin/forge.prev"
  },
  "schema": "forge.cli/v1"
}
```

When the CLI is already on the feed's version, `result` is
`{"current": …, "latest": …, "updated": false}` and nothing changes. The
table output adds a line on stderr: `Updated forge <old> → <new>. The old
binary is kept as <path>.` or `forge <version> is the latest release.`

Exit codes:

- `0`: updated, already up to date, or `--check` succeeded.
- `1`: you answered anything but `yes` at the prompt (`cancelled`), the
  signature does not match (`bad_signature`), the binary does not match the
  signed sums (`bad_checksum`), the signed sums do not list this platform's
  file, or the new binary could not be written or did not start
  (`install_failed`). The installed binary is unchanged.
- `2`: a feed URL that is not HTTPS (or HTTP to a loopback address), no
  release is built for this platform, or `--yes` missing without a terminal.
- `6`: the release has no signed file for this platform (`no_asset`).
- `7`: the feed's release is older than this CLI and you did not pass
  `--allow-downgrade` (`downgrade`).
- `9`: the feed or a download cannot be reached, answers with an error, is
  not a release document, or is larger than 200 MiB.

**See also**: [Install the CLI](/docs/reference/cli/#install),
[`forge system`](/docs/reference/cli/system/),
[Releases](/docs/development/releases/).
