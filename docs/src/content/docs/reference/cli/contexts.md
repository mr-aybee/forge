---
title: forge contexts
description: Add, list, switch, change and remove the named connection profiles the forge CLI uses to reach a Forge server.
sidebar:
  order: 1
---

A context is a named connection profile: a server URL, an optional extra CA
certificate and the stored login for that server. The CLI keeps contexts in
its settings directory; see
[Where the CLI keeps its session](/docs/reference/cli/#where-the-cli-keeps-its-session).
Every command takes the [global flags](/docs/reference/cli/#global-flags).

The context file (`config.json`) holds no secrets. Besides the server URL,
the CA file, the recorded workspace id and the signed-in user name, a
context keeps the local ports that
[`forge tunnel open`](/docs/reference/cli/tunnel/#forge-tunnel-open) picked for
each target (`tunnel_ports`), so a service lands on the same local port next
time. [`forge tunnel ports`](/docs/reference/cli/tunnel/#forge-tunnel-ports)
lists and forgets them.

These commands work on your machine only. `forge contexts remove` is the one
command that contacts the server, to revoke the stored login.

## Commands

| Command | What it does |
| --- | --- |
| [`forge contexts add`](#forge-contexts-add) | Add a context and make it current |
| [`forge contexts list`](#forge-contexts-list) | List contexts |
| [`forge contexts use`](#forge-contexts-use) | Switch the current context |
| [`forge contexts current`](#forge-contexts-current) | Show the current context |
| [`forge contexts update`](#forge-contexts-update) | Change a context's settings |
| [`forge contexts remove`](#forge-contexts-remove) | Remove a context and its stored login |

### forge contexts add

Adds a context and makes it the current one, unless you pass `--no-use`. It
needs no permission: it does not contact the server. Sign in afterwards with
[`forge auth login`](/docs/reference/cli/auth/#forge-auth-login).

**Synopsis**

```text
forge contexts add [OPTIONS] --server <SERVER> <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<name>` | string, up to 64 letters, digits, `.`, `_` or `-` | — | yes | Name of the new context | `lab` |
| `--server` | URL | — | yes | Server URL. Must be `https://`. `http://` works for `localhost` and loopback addresses only, unless `FORGE_ALLOW_HTTP=1` is set. No user name, password, query or fragment | `--server https://forge.example.com` |
| `--ca-file` | path | none | no | Extra CA certificate (PEM) for a server with a private or on-premises CA | `--ca-file ./forge-ca.pem` |
| `--no-use` | flag | off | no | Keep the current context. The first context you add still becomes current | `--no-use` |

:::caution
`FORGE_ALLOW_HTTP=1` lets the CLI send your tokens over plain HTTP. Use it
only in a lab without TLS, never across a network you do not control.
:::

**Examples**

```sh
# Add a context and make it current
forge contexts add lab --server https://forge.example.com

# Add a context for a server with a private CA
forge contexts add onprem --server https://<forge-host> --ca-file ./forge-ca.pem

# Add a second context without switching to it
forge contexts add prod --server https://<forge-host> --no-use
```

**Output**

The CLI prints a confirmation and the hint `Sign in with: forge auth login` on
stderr, then the result:

```json
{
  "name": "lab",
  "server": "https://forge.example.com"
}
```

With `-o json`, the same object sits in the `result` of the `forge.cli/v1`
envelope. Shape from the CLI source; not captured on a live system.

Exit codes:

- `0`: the context was added.
- `2`: invalid name or server URL (`invalid_input`).
- `7`: a context with that name exists; use `forge contexts update`.

**See also**: [`forge contexts update`](#forge-contexts-update),
[`forge auth login`](/docs/reference/cli/auth/#forge-auth-login),
[Sign in to the CLI on a server or over SSH](/docs/guides/cli-headless-login/).

### forge contexts list

Lists every context, marks the current one with `*`, and shows whether a login
is stored for it. It reads local settings only and needs no permission.

**Synopsis**

```text
forge contexts list [OPTIONS]
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
# List contexts
forge contexts list

# List contexts as JSON, for scripts
forge -o json contexts list
```

**Output**

```text
   NAME  SERVER                     USER   SIGNED IN
*  lab   https://forge.example.com  admin  true
```

```json
{
  "result": [
    {
      "current": "*",
      "instance_id": "01900000-0000-7000-8000-000000000001",
      "name": "lab",
      "server": "https://forge.example.com",
      "signed_in": true,
      "user": "admin"
    }
  ],
  "schema": "forge.cli/v1"
}
```

`signed_in` is `true` when a login is stored for the context's current
server. `instance_id` is the workspace id the CLI recorded at sign-in.

Exit codes:

- `0`: success, also when there are no contexts.

**See also**: [`forge contexts current`](#forge-contexts-current),
[`forge auth status`](/docs/reference/cli/auth/#forge-auth-status).

### forge contexts use

Makes a context the current one. Commands without `-C/--context` then use
it. It needs no permission.

**Synopsis**

```text
forge contexts use [OPTIONS] <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<name>` | string | — | yes | Context to make current | `prod` |

**Examples**

```sh
# Switch to the prod context
forge contexts use prod

# Run one command against another context without switching
forge -C lab auth whoami
```

**Output**

```json
{
  "current": "prod"
}
```

Shape from the CLI source; not captured on a live system.

Exit codes:

- `0`: the context is now current.
- `6`: no context with that name.

**See also**: [`forge contexts list`](#forge-contexts-list),
[global flags](/docs/reference/cli/#global-flags).

### forge contexts current

Shows the current context: its name, server, signed-in user and the workspace
id recorded at sign-in. It reads local settings only.

**Synopsis**

```text
forge contexts current [OPTIONS]
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
# Show the current context
forge contexts current

# Print only the server URL
forge -o json contexts current | jq -r .result.server
```

**Output**

```text
{
  "instance_id": "01900000-0000-7000-8000-000000000001",
  "name": "lab",
  "server": "https://forge.example.com",
  "user": "admin"
}
```

```json
{
  "result": {
    "instance_id": "01900000-0000-7000-8000-000000000001",
    "name": "lab",
    "server": "https://forge.example.com",
    "user": "admin"
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: success.
- `6`: no current context.

**See also**: [`forge contexts use`](#forge-contexts-use),
[`forge auth whoami`](/docs/reference/cli/auth/#forge-auth-whoami).

### forge contexts update

Changes a context's server, CA certificate or default cluster. It needs no
permission.

**Synopsis**

```text
forge contexts update [OPTIONS] <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<name>` | string | — | yes | Context to change | `lab` |
| `--server` | URL | unchanged | no | New server URL. Same rules as for `forge contexts add`. A different server removes the stored login, the user and the recorded workspace id; sign in again | `--server https://<forge-host>` |
| `--ca-file` | path | unchanged | no | New extra CA certificate (PEM) | `--ca-file ./forge-ca.pem` |
| `--default-cluster` | cluster id | unchanged | no | Cluster that `forge nodes list` filters by when you pass no `--cluster`. An empty value clears it | `--default-cluster <cluster-id>` |

**Examples**

```sh
# Point the context at a new server (you sign in again afterwards)
forge contexts update lab --server https://<forge-host>

# Set a default cluster
forge contexts update lab --default-cluster <cluster-id>

# Clear the default cluster
forge contexts update lab --default-cluster ""
```

**Output**

When the server changed, the CLI prints `The server changed: the stored login
was removed. Sign in again with: forge auth login` on stderr. Then:

```json
{
  "updated": "lab"
}
```

Shape from the CLI source; not captured on a live system.

Exit codes:

- `0`: the context was changed.
- `2`: invalid server URL.
- `6`: no context with that name.

**See also**: [`forge contexts add`](#forge-contexts-add),
[`forge auth login`](/docs/reference/cli/auth/#forge-auth-login).

### forge contexts remove

Revokes the context's session on the server, then deletes the stored login
and the context. When the removed context was current, no context is current
afterwards. Revoking your own session needs no permission.

**Synopsis**

```text
forge contexts remove [OPTIONS] <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<name>` | string | — | yes | Context to remove | `lab` |

**Examples**

```sh
# Remove a context and revoke its session
forge contexts remove lab
```

**Output**

```json
{
  "removed": "lab",
  "server_session_revoked": true
}
```

Shape from the CLI source; not captured on a live system.

:::note
The CLI removes the context even when it cannot revoke the session, for
example when you were not signed in or the server was unreachable. It then
prints `Note: the server session was not revoked` on stderr and
`server_session_revoked` is `false`. The session on the server then stays
valid until it expires.
:::

Exit codes:

- `0`: the context was removed, whether or not the server session was revoked.
- `6`: no context with that name.

**See also**: [`forge auth logout`](/docs/reference/cli/auth/#forge-auth-logout),
[`forge contexts list`](#forge-contexts-list).
