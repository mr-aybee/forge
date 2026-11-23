---
title: forge connectors
description: List and revoke AI app connections made through MCP, and the OAuth clients those apps registered, from the forge CLI.
sidebar:
  order: 15
---

An AI app (connector) reaches Forge through the MCP endpoint. It registers an
OAuth client, then each sign-in as a user creates a connection with its own
tokens and mode. These commands list and revoke both. For how connectors work,
see [MCP connectors](/docs/concepts/mcp-connectors/); to connect one, see
[Connect AI agents](/docs/guides/connect-ai-agents/). Every command takes the
[global flags](/docs/reference/cli/#global-flags).

Listing and revoking work even while connectors are turned off on the server.
Forge records each revocation in the audit log.

## Commands

| Command | What it does |
| --- | --- |
| [`forge connectors list`](#forge-connectors-list) | List your AI app connections, or everyone's with `--all` |
| [`forge connectors revoke`](#forge-connectors-revoke) | Disconnect one connection; its tokens stop working |
| [`forge connectors clients`](#forge-connectors-clients) | List registered OAuth clients |
| [`forge connectors revoke-client`](#forge-connectors-revoke-client) | Revoke a client and every connection made with it |

### forge connectors list

Lists AI app connections: the app, its mode, the host it redirected to, when
it was last used and when it was revoked. Without `--all` it lists the
connections you made and needs no special permission. `--all` lists
everyone's and needs `user.manage` on the platform (the `platform_admin`
role).

**Synopsis**

```text
forge connectors list [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--all` | flag | off | no | Everyone's connections, not only yours. Needs `user.manage` | `--all` |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row (this command's `--all` means everyone's, not every row) | `--limit 0` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |

**Examples**

```sh
# List your connections
forge connectors list

# List everyone's connections
forge connectors list --all

# Only the connections that still work
forge -o json connectors list | jq '.result[] | select(.revoked_at == null)'
```

**Output**

```text
ID                   APP            MODE       REDIRECT         USED           REVOKED
ac_6DENsJLeQfEB0VsG  Lab ask agent  ask        127.0.0.1:47123  1794576422487  1794576456240
ac_Kt47ezL5BJ61qT6R  Lab agent      auto       127.0.0.1:47123  1794572429621  1794572429675
ac_KWAX4sGXIGnVn1DM  Lab agent      read_only  127.0.0.1:47123  1794572429318  1794572429653
…
```

```json
{
  "result": [
    {
      "client_id": "mcp_z8SyOMVnQ9_4b_Gm-YtCe7Ux",
      "client_name": "Lab ask agent",
      "created_at": 1794576422475,
      "id": "ac_6DENsJLeQfEB0VsG",
      "last_used_at": 1794576422487,
      "mode": "ask",
      "redirect_host": "127.0.0.1:47123",
      "revoked_at": 1794576456240,
      "user_id": "01900000-0000-7000-8000-000000000003"
    },
    …
  ],
  "schema": "forge.cli/v1"
}
```

`mode` is `read_only`, `ask` or `auto` (see
[Modes](/docs/concepts/mcp-connectors/#modes)). Timestamps are epoch milliseconds;
`revoked_at` is `null` for a connection that still works.

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: `--all` without `user.manage`.

**See also**: [`forge connectors revoke`](#forge-connectors-revoke),
[`forge approvals list`](/docs/reference/cli/approvals/#forge-approvals-list).

### forge connectors revoke

Disconnects one connection. Its access and refresh tokens stop working at
once, and its pending approval requests are cancelled. You can revoke your
own connections; revoking someone else's needs `user.manage` on the platform
(the `platform_admin` role). The CLI asks for confirmation unless you pass
`--yes`.

**Synopsis**

```text
forge connectors revoke [OPTIONS] <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | connection id | — | yes | The connection's id from `forge connectors list` | `ac_6DENsJLeQfEB0VsG` |
| `--yes` | flag | off | no | Skip the confirmation. Required with `--non-interactive` or without a terminal | `--yes` |

**Examples**

```sh
# Disconnect one app connection; the CLI asks first
forge connectors revoke ac_6DENsJLeQfEB0VsG

# Disconnect without a prompt
forge connectors revoke ac_6DENsJLeQfEB0VsG --yes
```

**Output**

```json
{
  "revoked": "ac_6DENsJLeQfEB0VsG",
  "cancelled_approvals": 0
}
```

Shape from the server's schema; not captured on a live system.
`cancelled_approvals` counts the pending requests that were cancelled.

Exit codes:

- `0`: revoked.
- `1`: you did not answer `yes` at the prompt (`cancelled`).
- `2`: no `--yes` and no terminal to ask on.
- `4`: the connection is someone else's and you lack `user.manage`.
- `6`: no connection with that id.

**See also**: [`forge connectors revoke-client`](#forge-connectors-revoke-client),
[Revoke a connection](/docs/guides/connect-ai-agents/#revoke-a-connection).

### forge connectors clients

Lists the OAuth clients that AI apps registered through dynamic client
registration, with their redirect URIs. Needs `user.manage` on the platform
(the `platform_admin` role).

**Synopsis**

```text
forge connectors clients [OPTIONS]
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
# List registered clients
forge connectors clients

# Show each client's redirect URIs
forge -o json connectors clients | jq '.result[] | {name, redirect_uris}'
```

**Output**

```text
CLIENT                        NAME           USED  REVOKED
mcp_z8SyOMVnQ9_4b_Gm-YtCe7Ux  Lab ask agent  -     1794576456240
mcp_ukwHgrJ38qSvk3A7rQACx0hB  Lab agent      -     1794572429675
mcp_JpsN5NDmYH4Fd1M2OIUehePe  Lab agent      -     1794572415328
```

```json
{
  "result": [
    {
      "client_id": "mcp_z8SyOMVnQ9_4b_Gm-YtCe7Ux",
      "created_at": 1794576422414,
      "created_ip": null,
      "last_used_at": null,
      "name": "Lab ask agent",
      "redirect_uris": [
        "http://127.0.0.1:47123/callback"
      ],
      "revoked_at": 1794576456240
    },
    …
  ],
  "schema": "forge.cli/v1"
}
```

The client `name` is chosen by the app that registered it. Do not rely on it
to identify an app; check the redirect URIs and the connections made with it.

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: you lack `user.manage`.

**See also**: [`forge connectors revoke-client`](#forge-connectors-revoke-client),
[How sign-in works](/docs/concepts/mcp-connectors/#how-sign-in-works).

### forge connectors revoke-client

Revokes a registered OAuth client. Every connection made with it ends, their
tokens stop working, and their pending approval requests are cancelled. The
app cannot sign in with that client again. Needs `user.manage` on the platform
(the `platform_admin` role). The CLI asks for confirmation unless you pass
`--yes`.

**Synopsis**

```text
forge connectors revoke-client [OPTIONS] <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | client id | — | yes | The client's id from `forge connectors clients` | `mcp_z8SyOMVnQ9_4b_Gm-YtCe7Ux` |
| `--yes` | flag | off | no | Skip the confirmation. Required with `--non-interactive` or without a terminal | `--yes` |

:::caution
Revoking a client disconnects every user who connected that app, not only
you.
:::

**Examples**

```sh
# Revoke a client and all its connections; the CLI asks first
forge connectors revoke-client mcp_z8SyOMVnQ9_4b_Gm-YtCe7Ux

# Revoke without a prompt
forge connectors revoke-client mcp_z8SyOMVnQ9_4b_Gm-YtCe7Ux --yes
```

**Output**

```json
{
  "revoked": "mcp_z8SyOMVnQ9_4b_Gm-YtCe7Ux",
  "cancelled_approvals": 0
}
```

Shape from the server's schema; not captured on a live system.
`cancelled_approvals` counts the pending requests cancelled across all of the
client's connections.

Exit codes:

- `0`: revoked.
- `1`: you did not answer `yes` at the prompt (`cancelled`).
- `2`: no `--yes` and no terminal to ask on.
- `4`: you lack `user.manage`.
- `6`: no client with that id.

**See also**: [`forge connectors revoke`](#forge-connectors-revoke),
[Connect AI agents](/docs/guides/connect-ai-agents/).
