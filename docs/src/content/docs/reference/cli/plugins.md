---
title: forge plugins
description: Create publisher keys, sign plugin packages, and install, upgrade, enable, pin and remove plugins with the forge CLI.
sidebar:
  order: 10
---

These commands cover the plugin lifecycle on a Forge server and the publisher
tooling to sign a package. For how plugins are sandboxed and authorized, see
[Plugins](/docs/concepts/plugins/); for building one, see
[Writing plugins](/docs/guides/writing-plugins/). Every command takes the
[global flags](/docs/reference/cli/#global-flags).

`forge plugins keygen` and `forge plugins sign` work offline: they need no
server, context or login. All other commands talk to the server.

## Commands

| Command | What it does |
| --- | --- |
| [`forge plugins list`](#forge-plugins-list) | List installed plugins |
| [`forge plugins keygen`](#forge-plugins-keygen) | Generate a publisher signing key (offline) |
| [`forge plugins sign`](#forge-plugins-sign) | Sign a plugin package (offline) |
| [`forge plugins install`](#forge-plugins-install) | Install or upgrade a signed package, approving permissions |
| [`forge plugins enable`](#forge-plugins-enable) | Turn a plugin on |
| [`forge plugins disable`](#forge-plugins-disable) | Turn a plugin off |
| [`forge plugins pin`](#forge-plugins-pin) | Keep a plugin on its version |
| [`forge plugins unpin`](#forge-plugins-unpin) | Allow upgrades again |
| [`forge plugins remove`](#forge-plugins-remove) | Uninstall a plugin |

Every command that changes a plugin needs the `plugin.manage` action and an
MFA-verified session (see [`forge auth step-up`](/docs/reference/cli/auth/#forge-auth-step-up)).
The `platform_admin` role has it. Forge records each change, and each denial,
in the audit log.

### forge plugins list

Lists the installed plugins with their version, publisher, state and approved
permissions. Needs `view` on the platform.

**Synopsis**

```text
forge plugins list [OPTIONS]
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
# List installed plugins
forge plugins list

# Show the id and state of each plugin
forge -o json plugins list | jq -r '.result[] | "\(.plugin_id) \(.state)"'
```

**Output**

Captured on a server with no plugins installed:

```text
[]
```

```json
{
  "result": [],
  "schema": "forge.cli/v1"
}
```

Each installed plugin has this shape (shape from the server's schema; not
captured on a live system):

```json
{
  "plugin_id": "orbyn.uptime-probe",
  "version": "0.1.0",
  "publisher": "orbyn",
  "artifact_sha256": "<sha256-hex>",
  "state": "enabled",
  "pinned": false,
  "approved_permissions": ["host:log", "host:server-online"],
  "fault_count": 0,
  "quarantine_reason": null,
  "installed_at": 1794554225245,
  "updated_at": 1794554225245
}
```

`state` is `installed`, `enabled`, `disabled` or `quarantined`. Timestamps are
epoch milliseconds.

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: you lack `view` on the platform.

**See also**: [`forge plugins install`](#forge-plugins-install),
[Plugins](/docs/concepts/plugins/).

### forge plugins keygen

Generates an Ed25519 publisher signing key and writes it to a new file that
only your account can read. It works offline and needs no permission. It
refuses to overwrite an existing file.

**Synopsis**

```text
forge plugins keygen [OPTIONS] <OUT>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<out>` | path | — | yes | File to write the private key to (PKCS#8). Must not exist | `./acme-publisher.key` |

:::caution
The key file is the publisher's private key. Anyone who has it can sign
plugins that every Forge trusting your publisher installs. Keep it out of
version control and off shared machines.
:::

**Examples**

```sh
# Create a publisher key
forge plugins keygen ./acme-publisher.key

# Create a key and print only the public key, for the Forge administrator
forge -o json plugins keygen ./acme-publisher.key | jq -r .result.public_key
```

**Output**

```json
{
  "public_key": "<base64url-public-key>",
  "key_file": "./acme-publisher.key"
}
```

Shape from the CLI source; not captured on a live system. Give `public_key`
to the Forge platform administrator. They add it under `trusted_publishers`
in `forge.toml` (see [Configuration](/docs/reference/configuration/)) and restart
the server.

Exit codes:

- `0`: the key was written.
- `1`: the file could not be written.
- `7`: the file exists.

**See also**: [`forge plugins sign`](#forge-plugins-sign),
[Writing plugins](/docs/guides/writing-plugins/#4-sign-the-plugin).

### forge plugins sign

Signs a plugin package with a publisher key and prints the base64url
signature. It checks that the manifest parses first. It works offline and
needs no permission.

**Synopsis**

```text
forge plugins sign [OPTIONS] --key <KEY> --manifest <MANIFEST> --artifact <ARTIFACT>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--key` | path | — | yes | Publisher key file from `forge plugins keygen` | `--key ./acme-publisher.key` |
| `--manifest` | path | — | yes | Plugin manifest (TOML). See [Plugin manifest](/docs/reference/plugin-manifest/) | `--manifest forge-plugin.toml` |
| `--artifact` | path | — | yes | The component (`.wasm`, or `.wat` text) | `--artifact plugin.wasm` |

**Examples**

```sh
# Sign a package and print the signature
forge plugins sign --key ./acme-publisher.key --manifest forge-plugin.toml --artifact plugin.wasm

# Keep the signature in a variable for the install step
SIG=$(forge plugins sign --key ./acme-publisher.key --manifest forge-plugin.toml --artifact plugin.wasm)
```

**Output**

The table output is the signature on one line:

```text
<base64url-signature>
```

With `-o json`, the signature is the string in `result`. Shape from the CLI
source; not captured on a live system.

The signature covers the exact bytes of the manifest and the artifact.
Changing either file after signing makes the install fail.

Exit codes:

- `0`: signed.
- `1`: a file cannot be read, the key is not a valid publisher key, or the
  manifest does not parse.

**See also**: [`forge plugins install`](#forge-plugins-install),
[`forge plugins keygen`](#forge-plugins-keygen).

### forge plugins install

Uploads a signed package and installs it, or with `--upgrade` replaces an
installed plugin with a newer version. The plugin starts in the `installed`
state; turn it on with `forge plugins enable`. Needs `plugin.manage` on the
platform (on the plugin for `--upgrade`) and an MFA-verified session.

The server checks that the manifest's `forge_api` range fits the server, that
the signature comes from a publisher in `trusted_publishers`, that every
approved permission is one the manifest requests, and that every dependency
is installed in a matching version.

**Synopsis**

```text
forge plugins install [OPTIONS] --manifest <MANIFEST> --artifact <ARTIFACT> --signature <SIGNATURE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--manifest` | path | — | yes | Plugin manifest (TOML) | `--manifest forge-plugin.toml` |
| `--artifact` | path | — | yes | The component (`.wasm`, or `.wat` text). The CLI sends it base64-encoded | `--artifact plugin.wasm` |
| `--signature` | string (base64url) | — | yes | Signature from `forge plugins sign` | `--signature "$SIG"` |
| `--approve` | string, repeatable | none | no | A permission from the manifest's `requested` list to grant. Repeat the flag for each permission. Permissions you do not approve stay denied | `--approve host:log` |
| `--upgrade` | flag | off | no | Replace the installed plugin with the same id. The version must be higher, the publisher the same, and the plugin not pinned. The approved permissions are replaced by the `--approve` list | `--upgrade` |

:::caution
Approve only the permissions the plugin needs. An approved permission lets the
plugin's code call that host function on every invocation.
:::

**Examples**

```sh
# Install a package and grant one permission
forge plugins install --manifest forge-plugin.toml --artifact plugin.wasm \
  --signature "$SIG" --approve host:log

# Install with two permissions
forge plugins install --manifest forge-plugin.toml --artifact plugin.wasm \
  --signature "$SIG" --approve host:log --approve host:server-online

# Upgrade the installed plugin to the new version in these files
forge plugins install --upgrade --manifest forge-plugin.toml --artifact plugin.wasm \
  --signature "$SIG" --approve host:log --approve host:server-online
```

**Output**

The installed plugin, in the shape shown under
[`forge plugins list`](#forge-plugins-list), with `state` `installed` after a
new install. Shape from the server's schema; not captured on a live system.

Exit codes:

- `0`: installed or upgraded.
- `2`: the manifest is invalid, needs another plugin API version, approves a
  permission it does not request, or misses a dependency; or, with
  `--upgrade`, the version is not higher.
- `4`: you lack `plugin.manage`, the publisher is not trusted or the
  signature does not match, or, with `--upgrade`, the publisher differs.
- `5`: the session is not MFA-verified.
- `6`: with `--upgrade`, the plugin is not installed.
- `7`: the plugin is already installed (use `--upgrade`), or, with
  `--upgrade`, it is pinned.

**See also**: [`forge plugins enable`](#forge-plugins-enable),
[`forge plugins pin`](#forge-plugins-pin),
[Writing plugins](/docs/guides/writing-plugins/).

### forge plugins enable

Turns a plugin on. It also releases a quarantined plugin and resets its fault
count. Needs `plugin.manage` on the
plugin and an MFA-verified session.

**Synopsis**

```text
forge plugins enable [OPTIONS] <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | plugin id | — | yes | The manifest id of the plugin | `orbyn.uptime-probe` |

**Examples**

```sh
# Turn a plugin on
forge plugins enable orbyn.uptime-probe
```

**Output**

The plugin, in the shape shown under [`forge plugins list`](#forge-plugins-list),
with `state` `enabled`. Shape from the server's schema; not captured on a live
system.

Exit codes:

- `0`: enabled.
- `4`: you lack `plugin.manage`.
- `5`: the session is not MFA-verified.
- `6`: no plugin with that id.
- `7`: the plugin is already enabled.

**See also**: [`forge plugins disable`](#forge-plugins-disable),
[Plugins](/docs/concepts/plugins/#lifecycle).

### forge plugins disable

Turns a plugin off; it stays installed. Needs `plugin.manage` on the
plugin and an MFA-verified session.

**Synopsis**

```text
forge plugins disable [OPTIONS] <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | plugin id | — | yes | The manifest id of the plugin | `orbyn.uptime-probe` |

**Examples**

```sh
# Turn a plugin off
forge plugins disable orbyn.uptime-probe
```

**Output**

The plugin, in the shape shown under [`forge plugins list`](#forge-plugins-list),
with `state` `disabled`. Shape from the server's schema; not captured on a
live system.

Exit codes:

- `0`: disabled.
- `4`: you lack `plugin.manage`.
- `5`: the session is not MFA-verified.
- `6`: no plugin with that id.
- `7`: the plugin is not enabled.

**See also**: [`forge plugins enable`](#forge-plugins-enable),
[`forge plugins remove`](#forge-plugins-remove).

### forge plugins pin

Keeps a plugin on its current version: `forge plugins install --upgrade`
refuses it until you unpin it. Needs `plugin.manage` on the plugin and an
MFA-verified session.

**Synopsis**

```text
forge plugins pin [OPTIONS] <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | plugin id | — | yes | The manifest id of the plugin | `orbyn.uptime-probe` |

**Examples**

```sh
# Pin a plugin to its version
forge plugins pin orbyn.uptime-probe
```

**Output**

The plugin, in the shape shown under [`forge plugins list`](#forge-plugins-list),
with `pinned` `true`. Shape from the server's schema; not captured on a live
system.

Exit codes:

- `0`: pinned.
- `4`: you lack `plugin.manage`.
- `5`: the session is not MFA-verified.
- `6`: no plugin with that id.

**See also**: [`forge plugins unpin`](#forge-plugins-unpin),
[`forge plugins install`](#forge-plugins-install).

### forge plugins unpin

Allows upgrades of a pinned plugin again. Needs `plugin.manage` on the plugin
and an MFA-verified session.

**Synopsis**

```text
forge plugins unpin [OPTIONS] <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | plugin id | — | yes | The manifest id of the plugin | `orbyn.uptime-probe` |

**Examples**

```sh
# Allow upgrades again
forge plugins unpin orbyn.uptime-probe
```

**Output**

The plugin, in the shape shown under [`forge plugins list`](#forge-plugins-list),
with `pinned` `false`. Shape from the server's schema; not captured on a live
system.

Exit codes:

- `0`: unpinned.
- `4`: you lack `plugin.manage`.
- `5`: the session is not MFA-verified.
- `6`: no plugin with that id.

**See also**: [`forge plugins pin`](#forge-plugins-pin).

### forge plugins remove

Uninstalls a plugin and deletes its stored artifact. It never touches
infrastructure the plugin managed. Disable the plugin first. The CLI asks for
confirmation unless you pass `--yes`. Needs `plugin.manage` on the plugin and
an MFA-verified session.

**Synopsis**

```text
forge plugins remove [OPTIONS] <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | plugin id | — | yes | The manifest id of the plugin | `orbyn.uptime-probe` |
| `--yes` | flag | off | no | Skip the confirmation. Required with `--non-interactive` or without a terminal | `--yes` |

**Examples**

```sh
# Disable, then uninstall a plugin; the CLI asks first
forge plugins disable orbyn.uptime-probe
forge plugins remove orbyn.uptime-probe

# Uninstall in a script
forge --non-interactive plugins remove orbyn.uptime-probe --yes
```

**Output**

The server answers with no content: the table output is empty, and `-o json`
prints `{"result": null, "schema": "forge.cli/v1"}`. Shape from the CLI
source; not captured on a live system.

Exit codes:

- `0`: uninstalled.
- `1`: you did not answer `yes` at the prompt (`cancelled`).
- `2`: no `--yes` and no terminal to ask on.
- `4`: you lack `plugin.manage`.
- `5`: the session is not MFA-verified.
- `6`: no plugin with that id.
- `7`: the plugin is enabled, or another installed plugin depends on it.

**See also**: [`forge plugins disable`](#forge-plugins-disable),
[Plugins](/docs/concepts/plugins/).
