---
title: forge approvals
description: List, inspect and deny the changes your AI connectors asked for in Ask mode, from the forge CLI.
sidebar:
  order: 11
---

When an AI connector runs in **Ask** mode, every change it wants to make waits
as an approval request until you decide. These commands list, show and deny
those requests; approving happens in the console only. For how approvals work,
see [MCP connectors](/docs/concepts/mcp-connectors/#how-approvals-work); for the
console steps, see [Connect AI agents](/docs/guides/connect-ai-agents/#approve-what-an-agent-asks-for).
Every command takes the [global flags](/docs/reference/cli/#global-flags).

:::caution
Approving works only in the console, in your own browser session. An AI agent
that runs shell commands can use the CLI with your session, so a CLI approval
would let it approve its own request. The CLI can list and deny only, and
connector tokens cannot list or decide requests at all.
:::

## Commands

| Command | What it does |
| --- | --- |
| [`forge approvals list`](#forge-approvals-list) | List the requests your connectors made |
| [`forge approvals get`](#forge-approvals-get) | Show one request, with the exact call it would make |
| [`forge approvals deny`](#forge-approvals-deny) | Refuse a pending request; nothing runs |
| [`forge approvals approve`](#forge-approvals-approve) | Print the console link to approve a request |

### forge approvals list

Lists the requests your connectors made, newest first, up to 200. Any
signed-in user can list their own. `--all` lists everyone's and needs
`user.manage` on the platform (the `platform_admin` role).

**Synopsis**

```text
forge approvals list [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--status` | enum: `pending`, `approved`, `denied`, `executed`, `failed`, `expired`, `cancelled` | all | no | Only requests with this status. A request is `cancelled` when its connection was revoked while it waited | `--status pending` |
| `--all` | flag | off | no | Everyone's requests, not only yours. Needs `user.manage` | `--all` |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row (this command's `--all` means everyone's, not every row) | `--limit 0` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |

A request is `pending` until you decide or it expires. An approved request
runs once and ends as `executed` or `failed`.

**Examples**

```sh
# List your connectors' requests
forge approvals list

# Only the requests that wait for a decision
forge approvals list --status pending

# Everyone's failed requests
forge approvals list --all --status failed
```

**Output**

```text
ID                   STATUS    ACTION                      SUMMARY                                                       REQUESTER      EXPIRES
ap_F2EdU7t4Izo_v0Z0  executed  forge_stack_service_action  Start, stop or restart stack services: POST /stacks/01a1d51…  Lab ask agent  1794577322488
```

```json
{
  "result": [
    {
      "action": "forge_stack_service_action",
      "connection_id": "ac_6DENsJLeQfEB0VsG",
      "created_at": 1794576422488,
      "decided_at": 1794576426704,
      "decided_by": "01900000-0000-7000-8000-000000000003",
      "error": null,
      "expires_at": 1794577322488,
      "id": "ap_F2EdU7t4Izo_v0Z0",
      "kind": "mcp.tool",
      "mfa_required": false,
      "request": {
        "arguments": { … },
        "body": { … },
        "method": "POST",
        "uri": "/stacks/01900000-0000-7000-8000-000000000024/services"
      },
      "requester": "Lab ask agent",
      "result": { … },
      …
    }
  ],
  "schema": "forge.cli/v1"
}
```

Timestamps are epoch milliseconds. `requester` is the name the connector
registered with. `result` holds what the request returned after it ran, for
example the job it started.

Exit codes:

- `0`: success, also when nothing matches.
- `3`: not signed in.
- `4`: `--all` without `user.manage`, or a connector token was used.

**See also**: [`forge approvals get`](#forge-approvals-get),
[`forge connectors list`](/docs/reference/cli/connectors/#forge-connectors-list).

### forge approvals get

Shows one request, with the exact API call it would make (`request.method`,
`request.uri` and `request.body`) and the tool arguments the agent sent. You
can read your own requests; with `user.manage` on the platform you can read
anyone's.

**Synopsis**

```text
forge approvals get [OPTIONS] <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | approval id | — | yes | The request's id from `forge approvals list` | `ap_F2EdU7t4Izo_v0Z0` |

**Examples**

```sh
# Show one request
forge approvals get ap_F2EdU7t4Izo_v0Z0

# Print only the call it would make
forge -o json approvals get ap_F2EdU7t4Izo_v0Z0 | jq .result.request
```

**Output**

```text
{
  "action": "forge_stack_service_action",
  "connection_id": "ac_6DENsJLeQfEB0VsG",
  "created_at": 1794576422488,
  "decided_at": 1794576426704,
  "decided_by": "01900000-0000-7000-8000-000000000003",
  "error": null,
  "expires_at": 1794577322488,
  "id": "ap_F2EdU7t4Izo_v0Z0",
  "kind": "mcp.tool",
  "mfa_required": false,
  "request": {
    "arguments": {
      "action": "restart",
      "server_id": "01900000-0000-7000-8000-000000000006",
      "services": [
        "box"
      ],
      "stack": "01900000-0000-7000-8000-000000000024"
    },
    "body": {
      "action": "restart",
      "server_id": "01900000-0000-7000-8000-000000000006",
      "services": [
        "box"
      ]
    },
    "method": "POST",
    "uri": "/stacks/01900000-0000-7000-8000-000000000024/services"
  },
  "requester": "Lab ask agent",
  "result": {
    …
  },
  …
}
```

With `-o json`, the same object sits in the `result` of the `forge.cli/v1`
envelope.

:::note
The arguments come from an AI agent. Check the request itself, not the
agent's description of it, before you approve it in the console.
:::

Exit codes:

- `0`: success.
- `3`: not signed in.
- `6`: no request with that id that you can see.

**See also**: [`forge approvals deny`](#forge-approvals-deny),
[`forge approvals approve`](#forge-approvals-approve).

### forge approvals deny

Refuses a pending request; nothing runs. Only the person whose connector
asked can deny it, from the CLI or the console. The CLI shows the request's
summary and asks for confirmation unless you pass `--yes`. Forge records the
decision in the audit log.

**Synopsis**

```text
forge approvals deny [OPTIONS] <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | approval id | — | yes | The pending request to refuse | `ap_F2EdU7t4Izo_v0Z0` |
| `--yes` | flag | off | no | Skip the confirmation. Required with `--non-interactive` or without a terminal | `--yes` |

**Examples**

```sh
# Deny a request; the CLI shows its summary and asks first
forge approvals deny ap_F2EdU7t4Izo_v0Z0

# Deny without a prompt
forge approvals deny ap_F2EdU7t4Izo_v0Z0 --yes
```

**Output**

The request, in the shape shown under [`forge approvals get`](#forge-approvals-get),
with `status` `denied`. Shape from the server's schema; not captured on a live
system.

Exit codes:

- `0`: denied.
- `1`: you did not answer `yes` at the prompt (`cancelled`).
- `2`: no `--yes` and no terminal to ask on.
- `4`: the request belongs to someone else's connector, or a connector token
  was used.
- `6`: no request with that id that you can see.
- `7`: the request was already decided or has expired.

**See also**: [`forge approvals list`](#forge-approvals-list),
[`forge connectors revoke`](/docs/reference/cli/connectors/#forge-connectors-revoke).

### forge approvals approve

Does not approve. It prints the console link for the request and exits with
`5`, so an agent that runs the CLI cannot approve its own request. It does
not contact the server and needs no permission. In the console, only the
person whose connector asked can approve, and a request marked
`mfa_required` needs an MFA-verified browser session.

**Synopsis**

```text
forge approvals approve [OPTIONS] <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | approval id | — | yes | The request to open in the console | `ap_F2EdU7t4Izo_v0Z0` |

**Examples**

```sh
# Print the console link for a request
forge approvals approve ap_F2EdU7t4Izo_v0Z0
```

**Output**

```text
error: approve requests in the Forge console: https://forge.example.com/approvals?id=01900000-0000-7000-8000-000000000026 (the CLI can list and deny only, so an agent cannot approve its own request) [approval_in_console]
```

```json
{
  "error": {
    "class": "mfa_required",
    "code": "approval_in_console",
    "exit_code": 5,
    "message": "approve requests in the Forge console: https://forge.example.com/approvals?id=01900000-0000-7000-8000-000000000026 (the CLI can list and deny only, so an agent cannot approve its own request)",
    "request_id": null,
    "retryable": false
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `5`: always (`approval_in_console`). Open the printed link in your browser.
- `2`: no context and no `--server`.

**See also**: [Connect AI agents](/docs/guides/connect-ai-agents/#approve-what-an-agent-asks-for),
[MCP connectors](/docs/concepts/mcp-connectors/#how-approvals-work).
