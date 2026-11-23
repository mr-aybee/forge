---
title: forge parity
description: Compare the server's API with this CLI and see, for every operation, whether a typed command covers it, only forge api reaches it, or it is excluded.
sidebar:
  order: 27
---

`forge parity` shows how this `forge` binary covers the API of the server it
talks to. Use it to find out whether an operation has a typed command or
needs [`forge api call`](/docs/reference/cli/api/#forge-api-call). It takes the
[global flags](/docs/reference/cli/#global-flags).

## Commands

| Command | What it does |
| --- | --- |
| [`forge parity`](#forge-parity) | Classifies every API operation of the server |

### forge parity

Reads the server's OpenAPI document (`/api/openapi.json`) and puts each
operation in one class. You need to be signed in; no Cedar action is
checked.

| Class | Meaning | `CLI / REASON` column |
| --- | --- | --- |
| `command` | A typed command covers it | The command, such as `forge audit list` |
| `api` | Only `forge api call` reaches it | Why there is no typed command |
| `excluded` | The CLI does not use it on purpose, such as console sign-in or agent endpoints | Why it is excluded |
| `unclassified` | This CLI does not know the operation: the server is newer than the CLI | empty |

The classification ships inside the CLI, built from
`src/apps/forge-cli/parity.json`. A server test fails when a route is
missing from that file, so a CLI and server of the same release have no
`unclassified` operations. If you see some, update the CLI with
[`forge update`](/docs/reference/cli/update/).

**Synopsis**

```text
forge parity [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--gaps` | flag | off | no | Hide the operations a typed command covers. The totals still count them | `--gaps` |

**Examples**

```sh
# Every operation and how the CLI reaches it
forge parity
# Only what has no typed command
forge parity --gaps
# Operations this CLI does not know yet, for a script
forge -o json parity --gaps | jq -r '.result.operations[] | select(.class == "unclassified") | .operation'
```

**Output**

```text
$ forge parity --gaps
OPERATION                                                  CLASS     CLI / REASON
GET /.well-known/oauth-authorization-server                excluded  OAuth/MCP discovery metadata, read by clients automatically
GET /.well-known/oauth-protected-resource                  excluded  OAuth/MCP discovery metadata, read by clients automatically
GET /.well-known/oauth-protected-resource/mcp              excluded  OAuth/MCP discovery metadata, read by clients automatically
GET /agent/download/{os}/{arch}/{binary}                   excluded  agent binaries for the node installer
PUT /api/v1/services/{id}/health                       api       health checks are edited in the console (form or YAML); `fo…
POST /api/v1/services/{id}/health/validate             api       used by the console's health check editor; `forge api call`
POST /api/v1/approvals/{id}/approve                        excluded  approving is console-only so an agent cannot approve its ow…
GET /api/v1/audit/actions                                  api       action-type list for the console filter; `forge api call`
…
329 operations: 257 typed commands, 34 through `forge api call`, 38 excluded, 0 unclassified (server newer than this CLI)
```

Services (projects), service definitions, attached buckets, a store's
owner, single objects and the cluster ingress profile all have typed
commands: [`forge projects`](/docs/reference/cli/projects/),
[`forge services`](/docs/reference/cli/services/),
[`forge storage`](/docs/reference/cli/storage/) and
[`forge clusters ingress`](/docs/reference/cli/clusters/#forge-clusters-ingress-get).

The table cuts long reasons with `…`. The last line, with the totals, goes
to stderr.

```json
$ forge -o json parity --gaps
{
  "result": {
    "counts": {
      "api": 34,
      "command": 257,
      "excluded": 38
    },
    "operations": [
      {
        "action": null,
        "class": "excluded",
        "cli": "OAuth/MCP discovery metadata, read by clients automatically",
        "id": "connectors.authorization_server_metadata",
        "operation": "GET /.well-known/oauth-authorization-server"
      },
      {
        "action": "health.configure",
        "class": "api",
        "cli": "health checks are edited in the console (form or YAML); `forge api call`",
        "id": "app_health.update",
        "operation": "PUT /api/v1/services/{id}/health"
      },
      …
    ],
    …
  },
  "schema": "forge.cli/v1"
}
```

| Field | Description |
| --- | --- |
| `counts` | Operations per class. A class with no operations is left out |
| `total` | All operations, including those `--gaps` hides |
| `operations[].operation` | Method and path |
| `operations[].id` | The operation id for `forge api call` and `forge api describe` |
| `operations[].class` | `command`, `api`, `excluded` or `unclassified` |
| `operations[].cli` | The command, or the reason; empty for `unclassified` |
| `operations[].action` | The operation's Cedar action, or `null` when it checks none |

Exit codes:

- `0`: success.
- `3`: not signed in.
- `9`: the server cannot be reached.

**See also**: [`forge api`](/docs/reference/cli/api/),
[REST API](/docs/reference/rest-api/).
