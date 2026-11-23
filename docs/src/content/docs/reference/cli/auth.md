---
title: forge auth
description: Sign the forge CLI in and out, verify a one-time code for MFA-protected actions, and show who you are signed in as.
sidebar:
  order: 2
---

These commands sign the CLI in to the current [context](/docs/reference/cli/contexts/),
step the session up to MFA-verified, and sign it out. For sign-in on a machine
without a browser, see
[Sign in to the CLI on a server or over SSH](/docs/guides/cli-headless-login/); for
MFA and step-up, see [Multi-factor authentication](/docs/guides/mfa/). Every
command takes the [global flags](/docs/reference/cli/#global-flags).

## Commands

| Command | What it does |
| --- | --- |
| [`forge auth login`](#forge-auth-login) | Sign in with your browser, or from another device with `--device` |
| [`forge auth step-up`](#forge-auth-step-up) | Verify a one-time code so this session may run MFA-protected actions |
| [`forge auth logout`](#forge-auth-logout) | Revoke the session on the server and forget it locally |
| [`forge auth status`](#forge-auth-status) | Show whether a login is stored |
| [`forge auth whoami`](#forge-auth-whoami) | Show the signed-in user, roles and MFA state |

### forge auth login

Signs in to the current context and stores the login in the OS credential
store, or in an owner-only file where none is available (see
[Where the CLI keeps its session](/docs/reference/cli/#where-the-cli-keeps-its-session)).
It needs no permission, but it needs a context: add one first with
[`forge contexts add`](/docs/reference/cli/contexts/#forge-contexts-add).

By default the CLI opens your browser on the same machine and receives the
result on a local port (`127.0.0.1`), with OAuth authorization code and PKCE.
Password, MFA, passkeys and SSO happen in the browser. With `--device`, it
prints a URL (`<public_url>/device`) and an 8-letter code such as
`BCDF-GHJK`, then waits while you approve the code in the console on any
device. A device sign-in gets the MFA state of the browser session that
approves it.

**Synopsis**

```text
forge auth login [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--timeout-secs` | integer (seconds) | `600` | no | How long to wait for the sign-in. With `--device`, the code's lifetime of 600 seconds also caps the wait | `--timeout-secs 120` |
| `--device` | flag | off | no | Sign in from another device. For SSH sessions and machines without a browser | `--device` |

:::caution
With `--device`, check that the code in the console matches the code in your
terminal before you approve it. Approving a code you did not start signs
someone else's CLI in as you.
:::

**Examples**

```sh
# Sign in with the browser on this machine
forge auth login

# Sign in over SSH: approve the printed code from your workstation's browser
forge auth login --device

# Sign in to a context other than the current one
forge -C prod auth login
```

**Output**

The CLI prints a `Signed in to …` line on stderr, with the user and where it
kept the token, then:

```json
{
  "context": "lab",
  "user": "admin",
  "instance_id": "01900000-0000-7000-8000-000000000001",
  "mfa": true
}
```

Shape from the CLI source; not captured on a live system. `instance_id` is the
server's workspace id. When the context already recorded a different one, the
CLI discards the new login and exits `7` (`instance_mismatch`): the URL now
points at a different deployment.

Exit codes:

- `0`: signed in.
- `2`: no context, or `--non-interactive` is set. Interactive sign-in is
  refused in scripts; use `FORGE_TOKEN` there.
- `3`: the device sign-in was denied.
- `7`: the server is a different deployment than the context recorded
  (`instance_mismatch`).
- `8`: the wait ran out.
- `9`: the server is unreachable.

**See also**: [Sign in to the CLI on a server or over SSH](/docs/guides/cli-headless-login/),
[`forge auth step-up`](#forge-auth-step-up),
[`forge contexts add`](/docs/reference/cli/contexts/#forge-contexts-add).

### forge auth step-up

Verifies a one-time code from your authenticator app (TOTP), so the session
can run MFA-protected actions. It needs no permission, but it needs a stored
login from `forge auth login`. The account must have an authenticator app
set up.

**Synopsis**

```text
forge auth step-up [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--code` | string, 6 digits | prompt | no | The code from your authenticator app. Without it, the CLI asks on the terminal. With `--non-interactive` or no terminal, it fails with exit `5` (`step_up_required`) | `--code 123456` |

**Examples**

```sh
# Step up; the CLI asks for the code
forge auth step-up

# Step up in a script that has the code
forge --non-interactive auth step-up --code "$FORGE_OTP"
```

**Output**

```json
{
  "mfa": true
}
```

Shape from the CLI source; not captured on a live system.

A step-up replaces the session: the old access and refresh tokens stop
working, and the CLI stores the new pair. The refresh lifetime still counts
from the original sign-in.

:::note
A `FORGE_TOKEN` (or `--token`) can never be stepped up. The server would
revoke it and answer with a new token pair that the CLI cannot keep, so the
CLI refuses with exit `2` (`step_up_needs_context`) before it asks for a code. Sign in with a context and run
`forge auth step-up` instead. API keys cannot be stepped up either.
:::

**Automatic step-up.** When any command gets a `step_up_required` answer and
the CLI runs on a terminal with a stored login, it prints `This action needs
multi-factor authentication`, asks for a 6-digit code, steps up and retries
the request once. It never asks with `--non-interactive`, when stdin is not a
terminal, or with `FORGE_TOKEN`: the command exits with `5`. Run
`forge auth step-up` first in scripts that need MFA.

Step-up in the CLI works with an authenticator app only. An account with only
passkeys gets exit `2`; sign in with `forge auth login --device` and approve
from a browser session that is MFA-verified. Ten wrong codes lock step-up for
the account until 15 minutes after the first failure have passed.

Exit codes:

- `0`: the session is MFA-verified.
- `1`: too many wrong codes (`throttled`).
- `2`: `FORGE_TOKEN` or `--token` is set (`step_up_needs_context`), the account has no
  authenticator app, or the prompted code does not have 6 digits.
- `3`: the code is wrong (`invalid_mfa`), or you are not signed in.
- `5`: no code and no terminal to ask on (`step_up_required`).

**See also**: [Multi-factor authentication](/docs/guides/mfa/),
[Run MFA-protected actions](/docs/guides/cli-headless-login/#run-mfa-protected-actions),
[`forge auth whoami`](#forge-auth-whoami).

### forge auth logout

Revokes the session on the server and deletes the stored login for the
context. The context stays; sign in again with `forge auth login`. It needs no
permission.

**Synopsis**

```text
forge auth logout [OPTIONS]
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
# Sign out of the current context
forge auth logout

# Sign out of another context
forge -C prod auth logout
```

**Output**

```json
{
  "context": "lab",
  "server_session_revoked": true
}
```

Shape from the CLI source; not captured on a live system.
`server_session_revoked` is `false` when no login was stored or the server was
unreachable. The CLI deletes the local login in both cases.

Exit codes:

- `0`: signed out locally.
- `2`: no context selected.

**See also**: [`forge contexts remove`](/docs/reference/cli/contexts/#forge-contexts-remove),
[`forge auth status`](#forge-auth-status).

### forge auth status

Shows whether a login is stored for the context's server, or whether
`FORGE_TOKEN` is set when you target a server without a context. It does not
contact the server, so it does not tell you whether the session is still
valid; use [`forge auth whoami`](#forge-auth-whoami) for that.

**Synopsis**

```text
forge auth status [OPTIONS]
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
# Check the current context
forge auth status

# Sign in only when no login is stored
forge auth status >/dev/null 2>&1 || forge auth login
```

**Output**

```text
{
  "context": "lab",
  "instance_id": "01900000-0000-7000-8000-000000000001",
  "server": "https://forge.example.com",
  "signed_in": true
}
```

```json
{
  "result": {
    "context": "lab",
    "instance_id": "01900000-0000-7000-8000-000000000001",
    "server": "https://forge.example.com",
    "signed_in": true
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: a login is stored.
- `3`: not signed in (`not_signed_in`). The CLI prints only the error.

**See also**: [`forge auth whoami`](#forge-auth-whoami),
[`forge contexts list`](/docs/reference/cli/contexts/#forge-contexts-list).

### forge auth whoami

Asks the server who you are: the user, the role bindings and whether the
session is MFA-verified. Any signed-in user can run it.

**Synopsis**

```text
forge auth whoami [OPTIONS]
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
# Show the signed-in user
forge auth whoami

# Check in a script whether the session is MFA-verified
forge -o json auth whoami | jq .result.mfa
```

**Output**

```text
user      admin
name      Administrator
email     -
mfa       verified
role      platform_admin on platform:root
```

```json
{
  "result": {
    "bindings": [
      {
        "created_at": 1794554225245,
        "id": "01900000-0000-7000-8000-000000000002",
        "role": "platform_admin",
        "scope": {
          "id": "root",
          "kind": "platform"
        },
        "user_id": "01900000-0000-7000-8000-000000000003"
      }
    ],
    "mfa": true,
    "mfa_enrolled": false,
    "session_kind": "bearer",
    "user": {
      "created_at": 1794554225243,
      "disabled": false,
      "display_name": "Administrator",
      "email": null,
      "id": "01900000-0000-7000-8000-000000000003",
      "oidc_subject": null,
      "username": "admin"
    }
  },
  "schema": "forge.cli/v1"
}
```

The table shows each role binding's scope as `kind:id`. `mfa` is whether this session is MFA-verified; `mfa_enrolled` is
whether the account has an authenticator app.

Exit codes:

- `0`: success.
- `3`: not signed in, or the session ended.
- `9`: the server is unreachable.

**See also**: [`forge auth status`](#forge-auth-status),
[Roles and access](/docs/guides/roles-and-access/).
