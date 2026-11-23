---
title: forge users
description: List Forge accounts, create local accounts with a password from stdin, and remove a user's passkeys after a lost device.
sidebar:
  order: 12
---

`forge users` manages the people who can sign in to Forge. To give an account
a role, use [`forge access`](/docs/reference/cli/access/); for the full workflow,
see [Roles and access](/docs/guides/roles-and-access/). Every command takes the
[global flags](/docs/reference/cli/#global-flags).

## Commands

| Command | What it does |
| --- | --- |
| [`forge users list`](#forge-users-list) | Lists every account |
| [`forge users create`](#forge-users-create) | Creates a local account; the password comes from stdin |
| [`forge users reset-passkeys`](#forge-users-reset-passkeys) | Removes all passkeys of a user who lost a device |

### forge users list

Lists every account in Forge. Needs `user.manage` on the platform, which
platform administrators hold.

**Synopsis**

```text
forge users list
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

**Examples**

```sh
# Every account, as a table
forge users list
# Usernames of disabled accounts, for a script
forge -o json users list | jq -r '.result[] | select(.disabled) | .username'
```

**Output**

```text
$ forge users list
ID                                    USERNAME  EMAIL  NAME           DISABLED
01900000-0000-7000-8000-000000000003  admin     -      Administrator  false
```

```json
$ forge -o json users list
{
  "result": [
    {
      "created_at": 1794554225243,
      "disabled": false,
      "display_name": "Administrator",
      "email": null,
      "id": "01900000-0000-7000-8000-000000000003",
      "oidc_subject": null,
      "username": "admin"
    }
  ],
  "schema": "forge.cli/v1"
}
```

`created_at` is in epoch milliseconds. `oidc_subject` is set for accounts that
sign in through [SSO](/docs/guides/sso-oidc/).

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: you lack `user.manage` on the platform.

**See also**: [`forge users create`](#forge-users-create),
[`forge access list`](/docs/reference/cli/access/#forge-access-list),
[Roles and access](/docs/guides/roles-and-access/).

### forge users create

Creates a local account with a password. Needs `user.manage` on the platform.
The new account has no roles until you grant one with
[`forge access grant`](/docs/reference/cli/access/#forge-access-grant).

The CLI reads the initial password from stdin, never from a flag, so it does
not end up in shell history or the process list. One trailing newline is
removed. When stdin is a terminal, the command refuses to run.

**Synopsis**

```text
forge users create [OPTIONS] --password-stdin <USERNAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<username>` | string | — | yes | Sign-in name: 3–64 characters, lowercase letters, digits, `.`, `_` or `-`, starting with a letter or digit. Stored in lowercase | `alice` |
| `--email <EMAIL>` | string | none | no | Email address, a second sign-in name. Must be unique | `--email alice@example.com` |
| `--display-name <DISPLAY_NAME>` | string | `""` | no | Name shown in the console. When empty, Forge uses the username | `--display-name "Alice Example"` |
| `--password-stdin` | flag | — | yes | Read the initial password from stdin. At least 12 characters | `--password-stdin < alice.pw` |

**Examples**

```sh
# Create an account with the password from a file
forge users create alice --password-stdin < alice.pw
# Create an account with an email and display name; the password comes from an environment variable
printf '%s' "$ALICE_PASSWORD" | forge users create alice --email alice@example.com --display-name "Alice Example" --password-stdin
```

**Output**

The command prints the new account. Shape from the server's schema; not
captured on a live system.

```json
$ forge -o json users create alice --email alice@example.com --password-stdin < alice.pw
{
  "result": {
    "created_at": 1794554225243,
    "disabled": false,
    "display_name": "alice",
    "email": "alice@example.com",
    "id": "<user-id>",
    "oidc_subject": null,
    "username": "alice"
  },
  "schema": "forge.cli/v1"
}
```

Without `-o json`, the same object prints as a key/value list.

Exit codes:

- `0`: the account was created.
- `2`: stdin is a terminal or empty, the password is shorter than 12
  characters, the username or email is invalid, or the username or email is
  already in use.
- `3`: not signed in.
- `4`: you lack `user.manage` on the platform.

**See also**: [`forge access grant`](/docs/reference/cli/access/#forge-access-grant),
[Roles and access](/docs/guides/roles-and-access/).

### forge users reset-passkeys

Removes every passkey of a user, for example after they lost the device that
held them. It also ends all of that user's sessions, since a session may come
from a stolen passkey. Needs `user.manage` on the platform and an MFA-verified
session (step-up). The command asks for confirmation first.

**Synopsis**

```text
forge users reset-passkeys [OPTIONS] <USER>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<user>` | username, email or id | — | yes | The user whose passkeys to remove. A name must match exactly one account | `alice` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` | `--yes` |

**Examples**

```sh
# Remove alice's passkeys after a prompt
forge users reset-passkeys alice
# The same in a script, after an MFA step-up
forge auth step-up
forge users reset-passkeys alice@example.com --yes --non-interactive
```

**Output**

The command prints how many passkeys it removed. Shape from the server's
schema; not captured on a live system.

```json
$ forge -o json users reset-passkeys alice --yes
{
  "result": { "removed": 2 },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: the passkeys were removed (`removed` can be `0`).
- `1`: you answered anything but `yes` at the prompt.
- `2`: the name matches more than one account, or `--yes` is missing without a
  terminal.
- `3`: not signed in.
- `4`: you lack `user.manage` on the platform.
- `5`: the action needs an MFA-verified session (`step_up_required`). In a
  terminal the CLI asks for a one-time code and retries; with
  `--non-interactive` or `FORGE_TOKEN`, run `forge auth step-up` first.
- `6`: no such user.

:::caution
The user can no longer sign in with a passkey. Make sure they have another way
in, such as a password and an authenticator app, before you run this.
:::

**See also**: [Passkeys: an administrator removes someone's passkeys](/docs/guides/passkeys/#an-administrator-removes-someones-passkeys),
[`forge auth`](/docs/reference/cli/auth/).
