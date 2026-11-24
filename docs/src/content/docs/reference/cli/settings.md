---
title: forge settings
description: Read the workspace settings and instance facts, and set or reset key/value settings that services can reference.
sidebar:
  order: 16
---

`forge settings` reads and changes the workspace's key/value settings.
Services can reference a setting as `${forge.settings.<key>}` in an
environment value. For the console workflow, see
[Profile and API keys](/docs/guides/settings-and-api-keys/). Every command takes
the [global flags](/docs/reference/cli/#global-flags).

## Commands

| Command | What it does |
| --- | --- |
| [`forge settings list`](#forge-settings-list) | Shows the settings, and with `-o json` the instance facts and sign-in settings |
| [`forge settings set`](#forge-settings-set) | Sets a setting |
| [`forge settings unset`](#forge-settings-unset) | Resets a setting to its default |
| [`forge settings server`](#forge-settings-server) | Shows the server settings: the public address and the effective `forge.toml` values |
| [`forge settings public-address get`](#forge-settings-public-address-get) | Shows the public address links use |
| [`forge settings public-address set`](#forge-settings-public-address-set) | Sets the public address |
| [`forge settings public-address clear`](#forge-settings-public-address-clear) | Clears it; links use `public_url` |

### forge settings list

Shows the stored settings. With `-o json` it also returns the instance facts
(version, public URLs, data directory) and the sign-in settings. Needs `view`
on the workspace.

**Synopsis**

```text
forge settings list
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
# The stored key/value settings
forge settings list
# The server's version and public URL
forge -o json settings list | jq '.result.instance | {version, public_url}'
```

**Output**

The table shows the stored values only:

```text
$ forge settings list
KEY                              VALUE
bootstrap.initial_password_user  01900000-0000-7000-8000-000000000003
```

```json
$ forge -o json settings list
{
  "result": {
    "auth": {
      "local_passwords": true,
      "mfa_required_for": [
        "certificate.use",
        "db.admin",
        "file.admin",
        …
        "terminal.open"
      ],
      "oidc": null,
      "users": 1,
      "users_with_mfa": 0
    },
    "instance": {
      "agent_public_url": "https://192.0.2.10:3843",
      "cookie_secure": false,
      "data_dir": "/var/lib/forge",
      "node_public_url": "https://forge.example.com",
      "public_url": "https://forge.example.com",
      "version": "0.1.1-dev.20261113.214755"
    },
    "values": [
      {
        "key": "bootstrap.initial_password_user",
        "value": "01900000-0000-7000-8000-000000000003"
      }
    ]
  },
  "schema": "forge.cli/v1"
}
```

| Field | Description |
| --- | --- |
| `auth.local_passwords` | Whether local passwords are accepted |
| `auth.mfa_required_for` | Cedar actions that need an MFA-verified session, read from the guardrail policy and sorted |
| `auth.oidc` | `null`, or the SSO provider's `issuer_url`, `client_id` and `auto_provision` |
| `auth.users`, `auth.users_with_mfa` | Number of accounts, and how many have an authenticator app enrolled |
| `instance.*` | Values from the server's [configuration](/docs/reference/configuration/) and its version |
| `values` | The stored settings, as `key` and `value` |

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: you lack `view` on the workspace.

**See also**: [`forge settings set`](#forge-settings-set),
[Configuration](/docs/reference/configuration/).

### forge settings set

Sets a setting, creating it or replacing its value. Needs `workspace.manage`
on the workspace (roles `platform_admin` and `workspace_admin`). The command
asks for confirmation first. Forge audits the change as `settings.update`.

**Synopsis**

```text
forge settings set [OPTIONS] <KEY> <VALUE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<key>` | string | — | yes | 1–64 characters: lowercase letters, digits, `_`, `.` or `-`, starting with a letter. Keys that start with `agents.` are refused | `company.name` |
| `<value>` | string | — | yes | One line, at most 4096 characters | `"Example Ltd"` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` | `--yes` |

**Examples**

```sh
# Set a value after a prompt
forge settings set company.name "Example Ltd"
# Set a value from a script, without a prompt
forge settings set support.email support@example.com --yes --non-interactive
```

**Output**

On success the command prints nothing. With `-o json` it prints
`{"result": null, "schema": "forge.cli/v1"}`. Not captured on a live system.

Exit codes:

- `0`: the setting was saved.
- `1`: you answered anything but `yes` at the prompt.
- `2`: the key or value is invalid, the key starts with `agents.`, or `--yes`
  is missing without a terminal.
- `3`: not signed in.
- `4`: you lack `workspace.manage`.

:::note
Settings under `agents.` control automatic agent updates. They have their own
command, [`forge agents auto-update`](/docs/reference/cli/agents/#forge-agents-auto-update), which needs
MFA. See [Update agents](/docs/guides/update-agents/).
:::

**See also**: [`forge settings unset`](#forge-settings-unset),
[Profile and API keys](/docs/guides/settings-and-api-keys/).

### forge settings unset

Removes a setting, so it falls back to its default. Needs `workspace.manage`
on the workspace. The command asks for confirmation first. Forge audits the
change as `settings.delete`.

**Synopsis**

```text
forge settings unset [OPTIONS] <KEY>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<key>` | string | — | yes | The setting to remove. Keys that start with `agents.` are refused | `company.name` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` | `--yes` |

**Examples**

```sh
# Remove a setting after a prompt
forge settings unset company.name
# Remove it without a prompt
forge settings unset company.name --yes
```

**Output**

On success the command prints nothing, also when the key was not set. With
`-o json` it prints `{"result": null, "schema": "forge.cli/v1"}`. Not
captured on a live system.

Exit codes:

- `0`: the setting is gone.
- `1`: you answered anything but `yes` at the prompt.
- `2`: the key starts with `agents.`, or `--yes` is missing without a
  terminal.
- `3`: not signed in.
- `4`: you lack `workspace.manage`.

**See also**: [`forge settings set`](#forge-settings-set),
[`forge settings list`](#forge-settings-list).

### forge settings server

Shows the server-level settings that the console lists under
**Administration → Settings**: the [public address](/docs/reference/configuration/#public-address)
and the read-only `forge.toml` values. Needs `view` on the platform (a
platform-scope role). Calls `GET /api/v1/server/settings`.

**Synopsis**

```text
forge settings server
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
forge -o json settings server | jq .result.public_address.effective
```

**Output**

```json
$ forge -o json settings server
{
  "result": {
    "config": {
      "agent_public_url": "https://192.0.2.10:3843",
      "cookie_secure": false,
      "node_public_url": "http://192.0.2.10:3880",
      "public_url": "http://192.0.2.10:3880",
      "trusted_proxies": ["192.0.2.20"]
    },
    "public_address": {
      "allowed_origins": ["http://192.0.2.10:3880", "https://forge.example.com"],
      "configured": "http://192.0.2.10:3880",
      "effective": "https://forge.example.com",
      "https": true,
      "setting": "https://forge.example.com",
      "source": "setting"
    }
  },
  "schema": "forge.cli/v1"
}
```

| Field | Description |
| --- | --- |
| `public_address.effective` | The address links use now |
| `public_address.source` | `setting` (the runtime setting) or `config` (`public_url`) |
| `public_address.setting` | The runtime setting, or `null` |
| `public_address.configured` | `public_url` from `forge.toml` |
| `public_address.https` | Whether links use HTTPS |
| `public_address.allowed_origins` | Browser origins the WebSocket and `/mcp` Origin checks accept |
| `config.*` | `public_url`, `node_public_url`, `agent_public_url`, `cookie_secure` and `[security] trusted_proxies` |

Exit codes: `0` success, `3` not signed in, `4` you lack `view` on the platform.

### forge settings public-address get

Shows the public address: the value in use, where it comes from, the
setting and the `forge.toml` fallback. Needs `view` on the platform.

**Synopsis**

```text
forge settings public-address get
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
forge settings public-address get
```

**Output**

```text
$ forge settings public-address get
EFFECTIVE                  SOURCE   SETTING                    CONFIGURED
https://forge.example.com  setting  https://forge.example.com  http://192.0.2.10:3880
```

Not captured on a live system.

### forge settings public-address set

Sets the public address. Needs `system.update` on the platform (role
`platform_admin`) and an MFA-verified session. Asks for confirmation first.
Applies at once. Audited as `server.public_address.update`.

**Synopsis**

```text
forge settings public-address set [OPTIONS] <URL>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<url>` | string | — | yes | An `http` or `https` origin: host and optional port. No path, query, fragment, user name or password; at most 255 characters | `https://forge.example.com` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` | `--yes` |

**Examples**

```sh
forge settings public-address set https://forge.example.com
forge settings public-address set https://forge.example.com --yes --non-interactive
```

**Output**

Prints the new public address in the same table as
[`get`](#forge-settings-public-address-get). Not captured on a live system.

Exit codes: `0` saved, `1` you did not confirm, `2` the URL is invalid (for
example it has a path), `3` not signed in, `4` you lack `system.update` or
the session is not MFA-verified (run `forge auth step-up`).

### forge settings public-address clear

Removes the setting, so links use `public_url` from `forge.toml`. Same
permission and confirmation as [`set`](#forge-settings-public-address-set).
Audited as `server.public_address.clear`.

**Synopsis**

```text
forge settings public-address clear [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` | `--yes` |

**Examples**

```sh
forge settings public-address clear --yes
```

**Output**

Prints the public address now in use, in the same table as
[`get`](#forge-settings-public-address-get). Not captured on a live system.

**See also**: [Publish Forge on a domain](/docs/guides/publish-forge-on-a-domain/),
[`forge-server public-address`](/docs/reference/cli/forge-server/#forge-server-public-address).
