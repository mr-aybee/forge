---
title: forge api
description: List, describe and call any operation of the Forge REST API from the server's OpenAPI document, including those without a typed command.
sidebar:
  order: 26
---

`forge api` reaches every operation in the server's OpenAPI document
(`/api/openapi.json`), including those that have no typed command yet. Use a
typed command where one exists; [`forge parity`](/docs/reference/cli/parity/) shows
which operations have one. For every endpoint, body and status code, see the
[REST API reference](/docs/reference/rest-api/). Every command takes the
[global flags](/docs/reference/cli/#global-flags).

## Commands

| Command | What it does |
| --- | --- |
| [`forge api operations`](#forge-api-operations) | Lists operations, filtered by tag or text |
| [`forge api describe`](#forge-api-describe) | Shows one operation: parameters, permission, MFA, transport and schemas |
| [`forge api call`](#forge-api-call) | Calls an operation by id, with path and query parameters and a JSON body |
| [`forge api request`](#forge-api-request) | Sends a raw request to a path |

The server authorizes and audits every call exactly as for the console: the
Cedar action and MFA requirement of each operation still apply. These
commands need you to be signed in.

**Body check.** Before `call` or `request` sends a JSON body, the CLI checks
it against the operation's schema from the server it talks to: types,
required fields, allowed values and unknown fields. A mismatch exits with `2`
(`invalid_body`), sends nothing, and lists each problem (up to 20) with its
field path, such as `` `targets[0].proto` must be one of "tcp", "udp" ``.
Operations whose schema is untyped are sent as they are. The server checks
every body again either way. `--no-validate` skips the client-side check.

**Idempotency.** Every `POST`, `PUT`, `PATCH` and `DELETE` carries an
`Idempotency-Key`, a random one per call unless you pass
`--idempotency-key`. The CLI retries a lost answer with the same key, so the
server runs the change at most once. See
[Idempotency-Key](/docs/reference/rest-api/#idempotency-key) and
[Retries and idempotency](/docs/reference/cli/#retries-and-idempotency).

### forge api operations

Lists the operations of the server's API: id, method, path, whether it needs
MFA, and a summary. With `-o json`, each operation also has its Cedar
`action` and its `transport`.

**Synopsis**

```text
forge api operations [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--tag <TAG>` | string | all tags | no | Only operations of this tag (API module) | `--tag servers` |
| `--search <SEARCH>` | string | none | no | Only operations whose method, path, id or summary contains this text, ignoring case | `--search passkey` |

**Examples**

```sh
# Every operation
forge api operations
# The operations of one API module
forge api operations --tag tenancy
# Operations that mention role bindings, with their Cedar action
forge -o json api operations --search role-bindings | jq -r '.result[] | "\(.id)  \(.action)"'
```

**Output**

```text
$ forge api operations --tag tenancy
ID                             METHOD  PATH                             MFA    SUMMARY
tenancy.list_environments       GET     /api/v1/environments               false  the environments the caller may view
tenancy.create_environment      POST    /api/v1/environments               false  a new environment in a cluster
tenancy.set_environment_cluster PUT     /api/v1/environments/{id}/cluster  false  move an environment (and its services) into another cluster
tenancy.list_bindings          GET     /api/v1/role-bindings            false  List bindings
tenancy.create_binding         POST    /api/v1/role-bindings            false  Create binding
tenancy.delete_binding         DELETE  /api/v1/role-bindings/{id}       false  Delete binding
tenancy.list_tenants           GET     /api/v1/tenants                  false  List tenants
tenancy.create_tenant          POST    /api/v1/tenants                  false  Create tenant
tenancy.list_users             GET     /api/v1/users                    false  List users
tenancy.create_user            POST    /api/v1/users                    false  Create user
tenancy.current_workspace      GET     /api/v1/workspace                false  The deployment's single workspace (D-016)
```

```json
$ forge -o json api operations --tag tenancy
{
  "result": [
    {
      "action": "view",
      "id": "tenancy.list_environments",
      "method": "GET",
      "mfa": false,
      "path": "/api/v1/environments",
      "summary": "the environments the caller may view",
      "transport": "json"
    },
    {
      "action": "workspace.manage",
      "id": "tenancy.create_environment",
      "method": "POST",
      "mfa": false,
      "path": "/api/v1/environments",
      "summary": "a new environment in a cluster",
      "transport": "json"
    },
    …
  ],
  "schema": "forge.cli/v1"
}
```

`transport` is `json`, `stream` (a byte download), `sse` or `websocket`.

Exit codes:

- `0`: success, also when no operation matches.
- `3`: not signed in.
- `6`: the server does not publish `/api/openapi.json`.
- `9`: the server cannot be reached.

**See also**: [`forge api describe`](#forge-api-describe),
[`forge parity`](/docs/reference/cli/parity/).

### forge api describe

Shows one operation as JSON: its method and path, parameters, request body,
responses, the Cedar action (`x-forge-action`), whether it needs MFA
(`x-forge-mfa`), its transport (`x-forge-transport`), and every component
schema it refers to, followed transitively. The output is always JSON, also
without `-o json`.

**Synopsis**

```text
forge api describe [OPTIONS] <OPERATION>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<operation>` | operation id | — | yes | The operation's id, from `forge api operations` | `tenancy.create_environment` |

**Examples**

```sh
# Everything about one operation
forge api describe tenancy.create_environment
# Only the request body schema, pretty-printed
forge api describe tenancy.create_environment| jq '.result.components.schemas'
```

**Output**

```text
$ forge api describe tenancy.create_environment
{"result":{"components":{"schemas":{"CreateEnvironment":{"properties":{"cluster_id":{"default":null,"description":"Required: an environment always lives in a cluster (D-056).","type":["string","null"]},"name":{"type":"string"},"protected":{"default":false,"type":"boolean"}},"required":["name"],"type":"object"}}},"method":"POST","operationId":"tenancy.create_environment","parameters":[],"path":"/api/v1/environments","requestBody":{"content":{"application/json":{"schema":{"$ref":"#/components/schemas/CreateNamespace"}}},"required":true},"responses":{"200":{"description":"Success"},"default":{"content":{"application/problem+json":{"schema":{"$ref":"#/components/schemas/Problem"}}},"description":"Error (RFC 9457 problem details)"}},"security":[{"bearer":[]},{"session":[]}],"summary":"a new environment in a cluster","tags":["tenancy"],"x-forge-action":"workspace.manage","x-forge-authz":"static","x-forge-handler":"tenancy::create_namespace","x-forge-mfa":false,"x-forge-schema":"partial","x-forge-transport":"json"},"schema":"forge.cli/v1"}
```

`-o json` prints the same document. The shared `Problem` error schema is left
out of `components`.

Exit codes:

- `0`: success.
- `3`: not signed in.
- `6`: no operation with that id. The error suggests ids from the same tag.

**See also**: [`forge api call`](#forge-api-call).

### forge api call

Calls an operation by its id. `-p` fills the path parameters, `-q` adds query
parameters, and `--body` sends a JSON body. A call that changes state (any
method but `GET`) asks for confirmation first. Needs whatever the operation
needs; see `forge api describe`.

**Synopsis**

```text
forge api call [OPTIONS] <OPERATION>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<operation>` | operation id | — | yes | The operation to call | `tenancy.list_tenants` |
| `-p`, `--param <PARAMS>` | `name=value`, repeatable | none | when the path has parameters | Fills the `{name}` part of the path. The value is URL-encoded | `-p id=<environment-id>` |
| `-q`, `--query <QUERY>` | `name=value`, repeatable | none | no | Adds a query parameter. The value is URL-encoded | `-q limit=20` |
| `--body <BODY>` | JSON, `@file`, or `@-` | none | no | The JSON body: inline, read from a file, or read from stdin | `--body @environment.json` |
| `--out-file <OUT_FILE>` | path | none | for `stream` operations | Writes a byte-stream answer (a download) to this file. The file must not exist yet | `--out-file backup.tar.gz` |
| `--idempotency-key <IDEMPOTENCY_KEY>` | string | a random key per call | no | The `Idempotency-Key` header of a change. Reuse it to retry safely after a lost answer: the server returns the stored result instead of running the change again | `--idempotency-key move-ns-<environment-id>` |
| `--no-validate` | flag | off | no | Send the body without checking it against the operation's schema | `--no-validate` |
| `--yes` | flag | off | no | Confirm a call that changes state. Required with `--non-interactive` | `--yes` |

**Examples**

```sh
# A read: list tenants
forge api call tenancy.list_tenants
# A read with a query parameter: the 20 newest audit entries
forge api call audit.list -q limit=20
# A change with a path parameter and an inline body: move an environment into another cluster
forge api call tenancy.set_environment_cluster-p id=<environment-id> --body '{"cluster_id": "<cluster-id>"}' --yes
# A change with the body from a file and a key you can reuse for a retry
forge api call tenancy.create_environment--body @environment.json --idempotency-key create-ns-web --yes
```

**Output**

The server's answer, as a key/value list, or inside `result` with `-o json`. An
answer with no content prints nothing (`"result": null` with `-o json`). For
a `stream` operation the command prints what it wrote:

```json
{ "schema": "forge.cli/v1", "result": { "file": "backup.tar.gz", "bytes": 1048576 } }
```

Shape from the CLI's source; not captured on a live system. The capture
under [`forge api request`](#forge-api-request) shows the `invalid_body`
error, which `call` returns the same way.

Exit codes:

- `0`: success.
- `1`: you answered anything but `yes` at the prompt.
- `2`: a missing path parameter, a `-p` or `-q` without `=`, a body that is
  not valid JSON or does not match the schema (`invalid_body`), a `stream`
  operation without `--out-file`, an `sse` or `websocket` operation (use its
  typed command), `--yes` missing without a terminal, or a `400`/`422` answer.
- `3`: not signed in.
- `4`: the server denied the action.
- `5`: the action needs an MFA-verified session (`step_up_required`). In a
  terminal the CLI asks for a one-time code and retries; with
  `--non-interactive` or `FORGE_TOKEN`, run `forge auth step-up` first.
- `6`: no operation with that id, or the item does not exist.
- `7`: the `--out-file` exists, or the server answered `409`.
- `9`: the server cannot be reached.

**See also**: [`forge api describe`](#forge-api-describe),
[REST API](/docs/reference/rest-api/),
[`forge parity`](/docs/reference/cli/parity/).

### forge api request

Sends a raw request to a path on the server. Use it when you know the
endpoint from the [REST API reference](/docs/reference/rest-api/). The body check
runs when the path and method match an operation in the OpenAPI document. A
request that changes state (any method but `GET`) asks for confirmation
first.

**Synopsis**

```text
forge api request [OPTIONS] <METHOD> <PATH>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<method>` | HTTP method | — | yes | `GET`, `POST`, `PUT`, `PATCH` or `DELETE`, in any case | `POST` |
| `<path>` | string | — | yes | The path, starting with `/`, with any query string | `/api/v1/servers` |
| `--body <BODY>` | JSON, `@file`, or `@-` | none | no | The JSON body: inline, read from a file, or read from stdin | `--body @-` |
| `--idempotency-key <IDEMPOTENCY_KEY>` | string | a random key per call | no | The `Idempotency-Key` header of a change. Reuse it to retry safely after a lost answer | `--idempotency-key create-ns-web` |
| `--no-validate` | flag | off | no | Send the body without checking it against the operation's schema | `--no-validate` |
| `--yes` | flag | off | no | Confirm a request that changes state. Required with `--non-interactive` | `--yes` |

**Examples**

```sh
# A read
forge api request GET /api/v1/servers
# A read with a query string
forge api request GET '/api/v1/audit?limit=20'
# A change with the body from stdin
echo '{"name": "web", "cluster_id": "<cluster-id>"}' | forge api request POST /api/v1/environments --body @- --yes
```

**Output**

The server's answer, as for [`forge api call`](#forge-api-call). This
capture shows the client-side body check refusing a body; nothing is sent:

```text
$ forge api request POST /api/v1/environments --body '{"name": 7, "extra": true}' --yes
error: the body does not match tenancy.create_environment's schema (`forge api describe tenancy.create_environment`): `name` must be string (got integer) [invalid_body]
```

```json
$ forge -o json api request POST /api/v1/environments --body '{"name": 7, "extra": true}' --yes
{
  "error": {
    "class": "invalid_input",
    "code": "invalid_body",
    "exit_code": 2,
    "message": "the body does not match tenancy.create_environment's schema (`forge api describe tenancy.create_environment`): `name` must be string (got integer)",
    "request_id": null,
    "retryable": false
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: success.
- `1`: you answered anything but `yes` at the prompt.
- `2`: an unknown method, a path that does not start with `/`, a body that
  is not valid JSON or does not match the schema (`invalid_body`), `--yes`
  missing without a terminal, or a `400`/`422` answer.
- `3`: not signed in.
- `4`: the server denied the action.
- `5`: the action needs an MFA-verified session (`step_up_required`). In a
  terminal the CLI asks for a one-time code and retries; with
  `--non-interactive` or `FORGE_TOKEN`, run `forge auth step-up` first.
- `6`: the path or item does not exist.
- `7`: the server answered `409`.
- `9`: the server cannot be reached.

**See also**: [`forge api call`](#forge-api-call),
[REST API](/docs/reference/rest-api/).
