---
title: forge vault
description: Every forge vault command for storing, changing, revealing and deleting credential vault items, with flags, permissions, output and exit codes.
sidebar:
  order: 22
---

`forge vault` keeps accounts and passwords in the workspace's credential vault.
For what the vault holds and who can open it, see
[Credential vault](/docs/guides/credential-vault/). Every command takes the
[global flags](/docs/reference/cli/#global-flags).

`<item>` is a vault item id or name. Names match without regard to case. A
name that matches more than one item fails with exit code `2` and lists the
matching ids.

Only the **Platform admin** role can use the vault. Everyone else, including
**Admin**, gets exit code `4`, and Forge audits the refused attempt.
Revealing a password also needs the `secret.read` action with an MFA-verified
session, and never works with an API key.

The CLI never takes a password from a flag. Pipe it on stdin with
`--password-stdin`, so it stays out of shell history and process lists. One
trailing newline is dropped.

## Commands

| Command | What it does |
| --- | --- |
| [`forge vault list`](#forge-vault-list) | Lists vault items, without passwords |
| [`forge vault add`](#forge-vault-add) | Stores a new item with a password read from stdin |
| [`forge vault update`](#forge-vault-update) | Changes an item, and rotates its password with `--password-stdin` |
| [`forge vault reveal`](#forge-vault-reveal) | Prints an item's password |
| [`forge vault delete`](#forge-vault-delete) | Deletes an item |

### forge vault list

Lists the vault items of the workspace: names, usernames and URLs, never
passwords. Needs the **Platform admin** role.

**Synopsis**

```text
forge vault list [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

**Examples**

```sh
# List vault items
forge vault list

# List the credentials of former object stores (tag object-storage)
forge -o json vault list | jq '.result[] | select(.tags | index("object-storage"))'
```

**Output**

The test system had no items:

```text
$ forge vault list
ID  NAME  USERNAME  URL  UPDATED
```

```json
$ forge -o json vault list
{
  "result": [],
  "schema": "forge.cli/v1"
}
```

Each element of `result` has the fields shown under
[`forge vault add`](#forge-vault-add). `UPDATED` is epoch milliseconds.

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: you are not a platform administrator.

**See also**: [`forge vault reveal`](#forge-vault-reveal),
[Credential vault](/docs/guides/credential-vault/).

### forge vault add

Stores a new item. The password is read from stdin and sealed; everything
else is stored as plain metadata. Needs the **Platform admin** role.

**Synopsis**

```text
forge vault add [OPTIONS] --password-stdin <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<name>` | string | — | yes | Item name, 1 to 100 characters | `db-01 root` |
| `--username <USERNAME>` | string | `""` | no | Account name, up to 200 characters | `--username postgres` |
| `--url <URL>` | string | `""` | no | Where the account is used: `http://`, `https://`, `ssh://` or `rdp://`, up to 500 characters | `--url ssh://<host>` |
| `--notes <NOTES>` | string | `""` | no | Free text, up to 4000 characters | `--notes "Break-glass account"` |
| `--password-stdin` | flag | — | yes | Read the password from stdin: 1 to 4096 bytes | `--password-stdin < pw.txt` |

**Examples**

```sh
# Store a password from an environment variable
printf '%s' "$DB_PASSWORD" | forge vault add "db-01 root" --username postgres --password-stdin

# Store a password from a file, with a URL and notes
forge vault add "web-01 admin" --username admin --url ssh://<host> \
  --notes "Local admin" --password-stdin < web-01.pw
```

**Output**

A throwaway item with a fake password, deleted right after:

```text
$ forge vault add docs-demo --username demo --password-stdin
{
  "created_at": 1794590589517,
  "created_by": {
    "id": "01900000-0000-7000-8000-000000000003",
    "type": "user"
  },
  "id": "01900000-0000-7000-8000-000000000040",
  "links": [],
  "name": "docs-demo",
  "notes": "",
  "rotated_at": 1794590589517,
  "tags": [],
  "updated_at": 1794590589517,
  "url": "",
  "username": "demo"
}
```

With `-o json` the same object is wrapped in
`{"result": …, "schema": "forge.cli/v1"}`. The response never holds the
password. Timestamps are epoch milliseconds.

Exit codes:

- `0`: the item is stored.
- `2`: stdin is a terminal or empty, or a field is invalid.
- `4`: you are not a platform administrator.

**See also**: [`forge vault update`](#forge-vault-update),
[Credential vault](/docs/guides/credential-vault/).

### forge vault update

Changes an item's name, username, URL or notes. With `--password-stdin` it
also replaces the password and sets `rotated_at`. Fields you leave out keep
their current value. The item's tags and links are kept. Needs the
**Platform admin** role.

**Synopsis**

```text
forge vault update [OPTIONS] <ITEM>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<item>` | vault item id or name | — | yes | The item to change | `"db-01 root"` |
| `--name <NAME>` | string | current value | no | New name, 1 to 100 characters | `--name "db-01 postgres"` |
| `--username <USERNAME>` | string | current value | no | New account name | `--username postgres` |
| `--url <URL>` | string | current value | no | New URL: `http://`, `https://`, `ssh://` or `rdp://` | `--url ssh://<host>` |
| `--notes <NOTES>` | string | current value | no | New notes | `--notes "Rotated after audit"` |
| `--password-stdin` | flag | off | no | Read a new password from stdin and rotate it | `--password-stdin < new.pw` |

**Examples**

```sh
# Rename an item
forge vault update "db-01 root" --name "db-01 postgres"

# Rotate the password from an environment variable
printf '%s' "$NEW_DB_PASSWORD" | forge vault update "db-01 postgres" --password-stdin
```

**Output**

The changed item, with the fields shown under
[`forge vault add`](#forge-vault-add). Shape from the server's schema; not
captured on a live system.

Exit codes:

- `0`: the item is changed.
- `2`: the name matches more than one item, a field is invalid, or
  `--password-stdin` was given with stdin a terminal or empty.
- `4`: you are not a platform administrator.
- `6`: no such item.

**See also**: [`forge vault reveal`](#forge-vault-reveal).

### forge vault reveal

Prints an item's password to stdout. Needs the **Platform admin** role and
`secret.read` on the workspace with an MFA-verified session. It never works
with an API key. Forge audits every reveal and every refused attempt.

:::caution
`reveal` writes the password to stdout. Do not run it where an AI agent, a
log, a CI job or a shared terminal reads the output. Pipe it straight into the
program that needs it.
:::

**Synopsis**

```text
forge vault reveal [OPTIONS] <ITEM>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<item>` | vault item id or name | — | yes | The item | `"db-01 root"` |

**Examples**

```sh
# Copy a password to the clipboard without showing it (Linux, X11)
forge vault reveal "db-01 root" | xclip -selection clipboard

# Pass a password to a program on stdin
forge vault reveal "db-01 root" | <program> --password-stdin
```

**Output**

Table output prints only the password, followed by a newline. This capture is
the fake password of a throwaway item:

```text
$ forge vault reveal docs-demo
not-a-real-password
```

With `-o json` the result is `{"password": "…"}`, wrapped in
`{"result": …, "schema": "forge.cli/v1"}`.

Exit codes:

- `0`: success.
- `2`: the name matches more than one item.
- `4`: you are not a platform administrator, lack `secret.read`, or used an
  API key.
- `5`: an MFA-verified session is needed. With `FORGE_TOKEN` the CLI cannot
  step up; sign in with a context and run `forge auth step-up`.
- `6`: no such item.

**See also**: [Credential vault](/docs/guides/credential-vault/).

### forge vault delete

Deletes an item and its sealed password. Asks for confirmation unless you
pass `--yes`. Needs the **Platform admin** role.

**Synopsis**

```text
forge vault delete [OPTIONS] <ITEM>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<item>` | vault item id or name | — | yes | The item to delete | `"db-01 root"` |
| `--yes` | flag | off | no | Skip the confirmation. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

:::caution
Before you delete an item, check that no service mounts it as a secret
file.
:::

**Examples**

```sh
# Delete an item (asks for confirmation)
forge vault delete "db-01 root"

# Delete an item in a script
forge vault delete "db-01 root" --yes
```

**Output**

The throwaway item from [`forge vault add`](#forge-vault-add). On success the
CLI prints nothing; with `-o json` it prints
`{"result": null, "schema": "forge.cli/v1"}`.

```text
$ forge vault delete docs-demo --yes
```

Exit codes:

- `0`: the item is deleted.
- `1`: you answered something other than `yes` at the prompt.
- `2`: the name matches more than one item, or no `--yes` in non-interactive
  use.
- `4`: you are not a platform administrator.
- `6`: no such item.

**See also**: [`forge vault list`](#forge-vault-list).
