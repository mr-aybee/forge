---
title: forge api-keys
description: List, create and delete personal API keys, the bearer tokens scripts use instead of an interactive sign-in, and revoke other users' keys as a platform admin.
sidebar:
  order: 14
---

`forge api-keys` manages personal API keys. A key acts as you, with
your roles, so scripts and CI jobs can call Forge without a browser sign-in.
A restricted key (`--allow`) acts with only the actions and scopes you list.
For the console workflow, see
[Profile and API keys](/docs/guides/settings-and-api-keys/). Every command takes
the [global flags](/docs/reference/cli/#global-flags).

## Commands

| Command | What it does |
| --- | --- |
| [`forge api-keys list`](#forge-api-keys-list) | Lists your API keys |
| [`forge api-keys create`](#forge-api-keys-create) | Creates a key and prints its secret once |
| [`forge api-keys delete`](#forge-api-keys-delete) | Deletes a key by id |
| [`forge api-keys revoke-all`](#forge-api-keys-revoke-all) | Revokes every key of one user |

Without `--user`, these commands only see and change the keys of the
signed-in user, and need no Cedar action. With `--user`, and for
`revoke-all`, they act on another user's keys and need `user.manage` on the
platform (**Platform admin**). Revoking needs no MFA, so removing access is
never held up.

### forge api-keys list

Lists your API keys. The secret is never shown again; the list shows its
prefix so you can tell keys apart.

**Synopsis**

```text
forge api-keys list [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--user <USER>` | id | you | no | List another user's keys instead (Platform admin) | `--user <user-id>` |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

**Examples**

```sh
# Your keys
forge api-keys list
# Another user's keys, as a platform admin
forge api-keys list --user <user-id>
# Keys that were never used, for a clean-up script
forge -o json api-keys list | jq -r '.result[] | select(.last_used_at == null) | .id'
```

**Output**

Captured for an account with no keys:

```text
$ forge api-keys list
ID  NAME  PREFIX  ELEVATED  RESTRICTED  EXPIRES  USED
```

```json
$ forge -o json api-keys list
{
  "result": [],
  "schema": "forge.cli/v1"
}
```

Each key in `result` has the fields shown under
[`forge api-keys create`](#forge-api-keys-create), without `secret`.
`EXPIRES` and `USED` are `expires_at` and `last_used_at`, in epoch
milliseconds. An empty `EXPIRES` (`null`) is a key that never expires.
`RESTRICTED` is the key's `restrictions`: empty for a key that acts with all
of your access, otherwise the scopes and actions it is limited to.

Exit codes:

- `0`: success.
- `3`: not signed in.

**See also**: [`forge api-keys create`](#forge-api-keys-create),
[`forge api-keys delete`](#forge-api-keys-delete).

### forge api-keys create

Creates an API key and prints its secret once. Forge keeps only a hash of the
secret. You must be signed in as a user: a request made with an API key
cannot create another key.

**Synopsis**

```text
forge api-keys create [OPTIONS] <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<name>` | string | — | yes | A label for the key, 1–80 characters | `ci-deploy` |
| `--expires-days <EXPIRES_DAYS>` | integer, days | `90` | no | Days until the key expires. Forge clamps the value to 1–3650 | `--expires-days 30` |
| `--never-expires` | flag | off | no | A key that never expires. Not with `--expires-days`. Such keys are flagged in the console | `--never-expires` |
| `--elevated` | flag | off | no | An elevated key: it may run MFA-guarded actions (terminal, exec, package installs, updates) without a prompt, for CI. Needs an MFA-verified session (`forge auth step-up`) unless MFA is off for the server, and `[security] allow_elevated_tokens = true` | `--elevated` |
| `--allow <SCOPE=ACTIONS>` | `<kind>:<id>=<action>[,<action>…]`, repeatable | none (all of your access) | no | Restrict the key to these actions at this scope and everything beneath it. `<kind>` is `workspace`, `cluster`, `project`, `environment`, `server` or `service`; for `cluster` you can give the name instead of the id. Action names come from `forge access catalog`. Repeat for more scopes, 1 to 50 in total. You must hold every listed action over each scope now | `--allow environment:<environment-id>=view,deploy,job.create` |

**Examples**

```sh
# A key that expires in 90 days (the default)
forge api-keys create backup-script
# A key for CI that expires in 30 days, saved straight to a file only you can read
(umask 077; forge -o json api-keys create ci-deploy --expires-days 30 | jq -r '.result.secret' > ci-deploy.key)
# A key that never expires (avoid where you can)
forge api-keys create legacy-cron --never-expires
# An elevated key for CI that may run MFA-guarded actions
forge auth step-up
forge api-keys create ci-release --expires-days 30 --elevated
# A key that may only see and deploy one environment
forge api-keys create ci-uat --expires-days 30   --allow environment:<environment-id>=view,deploy,job.create
# A read-only key for a dashboard, over two clusters (by name)
forge api-keys create grafana-read --allow cluster:uat=view --allow cluster:prod=view
```

**Output**

Captured with a throwaway key that was deleted right after; the secret is
replaced with `<secret>`.

```text
$ forge api-keys create docs-demo --expires-days 1
{
  "created_at": 1794590587469,
  "expires_at": 1794676987469,
  "id": "01900000-0000-7000-8000-000000000027",
  "last_used_at": null,
  "name": "docs-demo",
  "prefix": "fk_3f9a1c2b…",
  "secret": "<secret>",
  "user_id": "01900000-0000-7000-8000-000000000003"
}
Store this secret now; Forge shows it only once.
```

The last line goes to stderr and only appears with the table output. With
`-o json`, the same object is in `result`. The object also has `elevated`
and `restrictions`. `restrictions` is `null` for a key with all of your
access; for a restricted key it is the list you gave, for example:

```json
"restrictions": [
  { "scope": { "kind": "environment", "id": "<environment-id>" }, "actions": ["view", "deploy", "job.create"] }
]
```

A restricted key may act only when you may **and** one entry lists the
action at a scope that covers the resource. Anything else is refused with
`403` and the detail `outside what this token was given`, and lists show
only what the key may see. If your own access shrinks later, so does the
key's. See [Restricted tokens](/docs/concepts/authorization/#restricted-tokens).

:::caution
Anyone with the secret can act as you until the key expires or you delete
it. Store it in a secret manager or CI secret, pass it to `forge` through the
`FORGE_TOKEN` environment variable, and never commit it or put it in a URL.
:::

To use the key:

```sh
export FORGE_SERVER=https://<forge-host>
export FORGE_TOKEN="$(cat ci-deploy.key)"
forge nodes list
```

Exit codes:

- `0`: the key was created.
- `2`: the name is empty or longer than 80 characters, an `--allow` value
  is not `<kind>:<id>=<action>[,…]`, names an unknown action or a scope kind
  that cannot be assigned, or there are more than 50 entries.
- `3`: not signed in.
- `4`: the request was made with an API key, or `--allow` lists an action
  you do not hold over that scope (`escalation`).
- `6`: an `--allow` scope does not exist.

**See also**: [`forge api-keys delete`](#forge-api-keys-delete),
[Global flags](/docs/reference/cli/#global-flags),
[Profile and API keys](/docs/guides/settings-and-api-keys/).

### forge api-keys delete

Deletes one of your API keys. Scripts that use it stop working at once. The
command asks for confirmation first.

**Synopsis**

```text
forge api-keys delete [OPTIONS] <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | id | — | yes | The key's id, from `forge api-keys list` | `<key-id>` |
| `--user <USER>` | id | you | no | Delete another user's key (Platform admin) | `--user <user-id>` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` | `--yes` |

**Examples**

```sh
# Delete a key after a prompt
forge api-keys delete <key-id>
# Delete it without a prompt
forge api-keys delete <key-id> --yes
```

**Output**

Captured with the throwaway key from the example above. On success the
command prints nothing:

```text
$ forge api-keys delete 01900000-0000-7000-8000-000000000027 --yes

```

With `-o json` it prints `{"result": null, "schema": "forge.cli/v1"}`.

Exit codes:

- `0`: the key was deleted.
- `1`: you answered anything but `yes` at the prompt.
- `2`: `--yes` is missing without a terminal.
- `3`: not signed in.
- `4`: `--user` without Platform admin.
- `6`: no key with that id.

**See also**: [`forge api-keys list`](#forge-api-keys-list),
[Profile and API keys](/docs/guides/settings-and-api-keys/).

### forge api-keys revoke-all

Revokes every API key of one user, for example when their laptop or a CI
secret leaked. Their scripts stop working at once. Needs `user.manage` on the
platform (**Platform admin**). The command asks for confirmation first.

**Synopsis**

```text
forge api-keys revoke-all [OPTIONS] --user <USER>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--user <USER>` | id | — | yes | The user whose keys to revoke | `--user <user-id>` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` | `--yes` |

**Examples**

```sh
# Revoke all of a user's keys after a prompt
forge api-keys revoke-all --user <user-id>
```

**Output**

```text
$ forge api-keys revoke-all --user 01900000-0000-7000-8000-000000000003 --yes
{
  "revoked": 2
}
```

Audited as `apikey.revoke_all`. `forge-server reset-mfa` and removing all of
a user's passkeys revoke their keys too.

Exit codes:

- `0`: the keys were revoked (also when there were none).
- `1`: you answered anything but `yes` at the prompt.
- `2`: `--yes` is missing without a terminal.
- `3`: not signed in.
- `4`: not a Platform admin.

**See also**: [`forge api-keys list`](#forge-api-keys-list).
