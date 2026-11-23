---
title: forge service-accounts
description: Create, disable and enable service accounts, and issue and revoke their tokens, for CI and other automation.
sidebar:
  order: 15
---

`forge service-accounts` manages service accounts: non-person identities for
CI pipelines, schedules and other automation. A service account never signs
in; it uses tokens. It gets access only through access profiles
([`forge access assignments add`](/docs/reference/cli/access/#forge-access-assignments-add));
its owner is accountable for it but lends it no access. For the model, see
[Authorization](/docs/concepts/authorization/#service-accounts); for the task, see
[Access profiles and service accounts](/docs/guides/access-profiles/). Every command
takes the [global flags](/docs/reference/cli/#global-flags).

## Commands

| Command | What it does |
| --- | --- |
| [`forge service-accounts list`](#forge-service-accounts-list) | Lists service accounts |
| [`forge service-accounts create`](#forge-service-accounts-create) | Creates one |
| [`forge service-accounts disable`](#forge-service-accounts-disable) | Disables one and revokes its tokens |
| [`forge service-accounts enable`](#forge-service-accounts-enable) | Allows a disabled one again |
| [`forge service-accounts tokens`](#forge-service-accounts-tokens) | Lists its tokens |
| [`forge service-accounts token-create`](#forge-service-accounts-token-create) | Issues a token |
| [`forge service-accounts token-revoke`](#forge-service-accounts-token-revoke) | Revokes a token |

### forge service-accounts list

Lists service accounts with their owner, whether they're disabled, how many tokens they hold and when one was last used. In JSON, `owner_inactive` is true when the owner is disabled or gone. Needs `view` on the workspace.

**Synopsis**

```text
forge service-accounts list
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
forge service-accounts list
```

**Output**

```text
$ forge service-accounts list
ID                                    NAME           OWNER  DISABLED  TOKENS  USED
01900000-0000-7000-8000-000000000401  svc-ci-deploy  alice  false     1       1794594187469
```

Exit codes:

- `0`: success.
- `3`: not signed in.

**See also**: [`forge service-accounts tokens`](#forge-service-accounts-tokens).

### forge service-accounts create

Creates a service account named `svc-<name>`. It has no password, passkey, authenticator or SSO link, so it never signs in; it uses tokens. It can do nothing until you give it a profile with `forge access assignments add`. Needs `access.manage` (administrators) and an MFA-verified session.

**Synopsis**

```text
forge service-accounts create [OPTIONS] <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<name>` | string | — | yes | 3–40 of `a-z`, `0-9` and `-`; the account is `svc-<name>` | `ci-deploy` |
| `--description <DESCRIPTION>` | string | empty | no | What it is for, up to 500 characters | `--description "GitHub Actions deploys"` |
| `--owner <OWNER>` | username or email | you | no | The accountable person; must be a person | `--owner alice` |

**Examples**

```sh
forge service-accounts create ci-deploy --description "GitHub Actions deploys"
forge access assignments add svc-ci-deploy Developer environment:<environment-id> --yes
```

**Output**

Prints the account (one row of `forge service-accounts list`, in full).

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: not allowed (missing permission).
- `5`: an MFA-verified session is needed; step up and try again.
- `7`: `svc-<name>` exists.

**See also**: [`forge service-accounts token-create`](#forge-service-accounts-token-create).

### forge service-accounts disable

Disables a service account: all its tokens are revoked, and its sessions, terminals and event streams end at once. Enabling it again does not bring the tokens back. Needs `access.manage` and an MFA-verified session.

**Synopsis**

```text
forge service-accounts disable [OPTIONS] <ACCOUNT>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<account>` | id or `svc-<name>` | — | yes | The service account, by id or name (the `svc-` prefix may be left out) | `ci-deploy` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` | `--yes` |

**Examples**

```sh
forge service-accounts disable ci-deploy --yes
```

**Output**

Prints the account with `"disabled": true`.

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: not allowed (missing permission).
- `5`: an MFA-verified session is needed; step up and try again.
- `1`: you answered anything but `yes` at the prompt.
- `6`: no such service account.

**See also**: [`forge service-accounts enable`](#forge-service-accounts-enable).

### forge service-accounts enable

Allows a disabled service account again. Its old tokens stay revoked: issue new ones with `forge service-accounts token-create`. Needs `access.manage` and an MFA-verified session.

**Synopsis**

```text
forge service-accounts enable <ACCOUNT>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<account>` | id or `svc-<name>` | — | yes | The service account, by id or name (the `svc-` prefix may be left out) | `ci-deploy` |

**Examples**

```sh
forge service-accounts enable ci-deploy
```

**Output**

Prints the account with `"disabled": false`.

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: not allowed (missing permission).
- `5`: an MFA-verified session is needed; step up and try again.
- `6`: no such service account.

**See also**: [`forge service-accounts disable`](#forge-service-accounts-disable).

### forge service-accounts tokens

Lists an account's tokens: name, prefix, expiry and last use. Secrets are never shown again. Needs `view` on the workspace.

**Synopsis**

```text
forge service-accounts tokens <ACCOUNT>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<account>` | id or `svc-<name>` | — | yes | The service account, by id or name (the `svc-` prefix may be left out) | `ci-deploy` |

**Examples**

```sh
forge service-accounts tokens ci-deploy
```

**Output**

```text
$ forge service-accounts tokens ci-deploy
ID                                    NAME  PREFIX       EXPIRES        USED
01900000-0000-7000-8000-000000000501  gha   fk_3f9a1c2b  1802370187469  1794594187469
```

Exit codes:

- `0`: success.
- `3`: not signed in.
- `6`: no such service account.

**See also**: [`forge service-accounts token-revoke`](#forge-service-accounts-token-revoke).

### forge service-accounts token-create

Issues a token and prints its secret once; Forge keeps only a hash. At most 10 tokens per account and 20 issued per hour. Tokens never hold MFA, so MFA-guarded actions stay refused unless MFA is turned off for the server. Needs `access.manage` and an MFA-verified session.

**Synopsis**

```text
forge service-accounts token-create [OPTIONS] <ACCOUNT> <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<account>` | id or `svc-<name>` | — | yes | The service account, by id or name (the `svc-` prefix may be left out) | `ci-deploy` |
| `<name>` | string | — | yes | A label for the token, 1–80 characters | `gha` |
| `--expires-days <EXPIRES_DAYS>` | integer, days | `90` | no | Days until it expires, 1–3650 | `--expires-days 30` |
| `--never-expires` | flag | off | no | A token that never expires (flagged in lists). Not with `--expires-days` | `--never-expires` |
| `--elevated` | flag | off | no | An elevated token: it may run MFA-guarded actions without a prompt. Needs an MFA-verified session (`forge auth step-up`) unless MFA is off for the server, and `[security] allow_elevated_tokens = true` | `--elevated` |
| `--allow <SCOPE=ACTIONS>` | `<kind>:<id>=<action>[,<action>…]`, repeatable | all of the account's access | no | Restrict the token to these actions at these scopes, as for a personal API key (`forge api-keys create --allow`). It never acts with more than the account holds | `--allow service:<service-id>=view,deploy,job.create` |

**Examples**

```sh
(umask 077; forge -o json service-accounts token-create ci-deploy gha | jq -r '.result.secret' > gha.token)
# An elevated token for a release pipeline
forge auth step-up
(umask 077; forge -o json service-accounts token-create ci-deploy release --elevated | jq -r '.result.secret' > release.token)
```

**Output**

```text
$ forge service-accounts token-create ci-deploy gha
{
  "created_at": 1794590587469,
  "expires_at": 1802366587469,
  "id": "01900000-0000-7000-8000-000000000501",
  "last_used_at": null,
  "name": "gha",
  "prefix": "fk_3f9a1c2b",
  "secret": "<secret>",
  "user_id": "01900000-0000-7000-8000-000000000401"
}
Store this secret now; Forge shows it only once.
```

The last line goes to stderr and only appears with the table output.

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: not allowed (missing permission).
- `5`: an MFA-verified session is needed; step up and try again.
- `7`: the account is disabled or holds 10 tokens.
- `6`: no such service account.
- `1`: too many tokens issued this hour.

**See also**: [`forge service-accounts tokens`](#forge-service-accounts-tokens).

### forge service-accounts token-revoke

Revokes a token: requests with it fail at once, and terminals opened with it end. Needs `user.manage` on the workspace; no MFA, so removing access is never held up.

**Synopsis**

```text
forge service-accounts token-revoke [OPTIONS] <ACCOUNT> <TOKEN>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<account>` | id or `svc-<name>` | — | yes | The service account, by id or name (the `svc-` prefix may be left out) | `ci-deploy` |
| `<token>` | id | — | yes | From `forge service-accounts tokens` | `<token-id>` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` | `--yes` |

**Examples**

```sh
forge service-accounts token-revoke ci-deploy <token-id> --yes
```

**Output**

Prints nothing on success; with `-o json`, `{"result": null, "schema": "forge.cli/v1"}`.

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: not allowed (missing permission).
- `5`: an MFA-verified session is needed; step up and try again.
- `1`: you answered anything but `yes` at the prompt.
- `6`: no such account or token.

**See also**: [`forge service-accounts token-create`](#forge-service-accounts-token-create).
