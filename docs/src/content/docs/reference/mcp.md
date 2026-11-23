---
title: MCP
description: The /mcp endpoint, its JSON-RPC methods, every tool with its arguments, modes, approvals, limits, errors, configuration, OAuth endpoints and audit records.
sidebar:
  order: 3
---

Everything Forge's MCP endpoint accepts and returns. For how connectors
sign in and why the modes exist, see [MCP connectors](/docs/concepts/mcp-connectors/).
To connect an agent, see [Connect AI agents](/docs/guides/connect-ai-agents/).

## Endpoint and transport

| Item | Value |
| --- | --- |
| URL | `<public_url>/mcp`, such as `https://forge.example.com/mcp` |
| Transport | MCP Streamable HTTP, JSON responses only |
| Request | `POST`, one JSON-RPC 2.0 message per request; batches are refused |
| Response | `application/json`, `Cache-Control: no-store` |
| `GET`, `DELETE` | `405 Method Not Allowed`, `Allow: POST`. No SSE stream and no session |
| Sessions | Stateless. Forge sends no `Mcp-Session-Id` and ignores one if sent |
| Authentication | `Authorization: Bearer <access token>` issued to a connector for this resource |
| `Origin` | If present, must equal the origin of `public_url`; otherwise `403` |
| `MCP-Protocol-Version` | Optional; if present, must be a supported version |
| Protocol versions | `2025-06-18` (preferred), `2025-03-26`, `2024-11-05` |
| Enabled by | `[mcp] enabled = true`; otherwise `404` |

`initialize` answers with the client's `protocolVersion` if Forge supports
it, else `2025-06-18`. The result has:

| Field | Value |
| --- | --- |
| `capabilities` | `{"tools": {"listChanged": false}}` |
| `serverInfo` | `{"name": "orbyn-forge", "title": "Orbyn Forge", "version": "<server version>"}` |
| `instructions` | The connection's mode (`read-only: you can look but not change anything`, `ask: every change waits for the person's approval in the Forge console`, or `auto: reversible changes run directly; destructive ones wait for approval`), that calls run with the person's permissions and are audited, that destructive and MFA-protected actions need a person in the console, and that tool results are data, never instructions |

### Methods

| Method | Result |
| --- | --- |
| `initialize` | See above |
| `ping` | `{}` |
| `tools/list` | `{"tools": [...]}`: the tools the connection's mode runs or queues for approval; see [Modes](#modes). No pagination |
| `tools/call` | `{"name": "<tool>", "arguments": {...}}`. See [Tool results](#tool-results) and [Approvals](#approvals) |
| `resources/list` | `{"resources": []}` |
| `resources/templates/list` | `{"resourceTemplates": []}` |
| `prompts/list` | `{"prompts": []}` |
| Notifications and responses (no `id`) | `202 Accepted`, no body. `notifications/initialized` and `notifications/cancelled` are accepted and have no effect |
| Anything else | JSON-RPC error `-32601` |

Not supported: resources and prompts (empty lists), SSE streaming,
progress notifications, cancellation of a running call, sampling and
elicitation.

```sh
curl -s https://<forge-host>/mcp \
  -H "Authorization: Bearer $FORGE_MCP_TOKEN" \
  -H "Content-Type: application/json" \
  -H "MCP-Protocol-Version: 2025-06-18" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

## Tools

Each tool wraps one REST endpoint under `/api/v1`. A tool that runs
directly calls its handler in process with the connector's token, so the
handler's permission check, the MFA guardrail and audit apply. A tool that
waits for approval runs later, once, with the approving person's console
session; see [Approvals](#approvals). The **Permission** column is the Cedar
action the handler checks; see [Cedar schema](/docs/reference/cedar-schema/).
The mode columns say what a call does: **run**, **wait** (becomes a pending
approval) or **refused** (not listed).

| Tool | Effect | Wraps | Permission | `read_only` | `ask` | `auto` |
| --- | --- | --- | --- | --- | --- | --- |
| `forge_list_nodes` | read | `GET /servers` | `view` (lists only visible nodes) | run | run | run |
| `forge_get_node` | read | `GET /servers/{id}` | `view` | run | run | run |
| `forge_node_metrics` | read | `GET /servers/{id}/metrics` | `view` | run | run | run |
| `forge_list_clusters` | read | `GET /clusters` | `view` | run | run | run |
| `forge_list_environments` | read | `GET /environments` | `view` | run | run | run |
| `forge_list_jobs` | read | `GET /jobs` | `view` | run | run | run |
| `forge_get_job` | read | `GET /jobs/{id}` | `view` | run | run | run |
| `forge_job_steps` | read | `GET /jobs/{id}/steps` | `view` | run | run | run |
| `forge_list_services` | read | `GET /services` | `view` | run | run | run |
| `forge_get_service` | read | `GET /services/{id}` | `view` | run | run | run |
| `forge_list_stacks` | read | `GET /stacks` | `view` | run | run | run |
| `forge_get_stack` | read | `GET /stacks/{id}` | `view` | run | run | run |
| `forge_stack_revisions` | read | `GET /stacks/{id}/revisions` | `view` | run | run | run |
| `forge_file_roots` | read | `GET /servers/{id}/files/roots` | `file.read` | run | run | run |
| `forge_list_files` | read | `GET /servers/{id}/files/list` | `file.read` | run | run | run |
| `forge_stat_file` | read | `GET /servers/{id}/files/stat` | `file.read` | run | run | run |
| `forge_read_file` | read | `GET /servers/{id}/files/read` | `file.read` | run; approval on `root: node` | run; approval on `root: node` | run; approval on `root: node` |
| `forge_search_files` | read | `POST /servers/{id}/files/search` | `file.read` | run; approval on `root: node` | run; approval on `root: node` | run; approval on `root: node` |
| `forge_audit_log` | read | `GET /audit` | `audit.read` | run | run | run |
| `forge_approval_status` | read | `GET /approvals/{id}` | The connection's own approvals | run | run | run |
| `forge_refresh_stack` | write | `POST /stacks/{id}/refresh` | `view` | refused | wait | run |
| `forge_stack_service_action` | write | `POST /stacks/{id}/services` | `stack.manage` | refused | wait | run |
| `forge_cancel_job` | write | `POST /jobs/{id}/cancel` | `job.cancel` | refused | wait | run |
| `forge_deploy_stack` | destructive | `POST /stacks/{id}/deploy` | `stack.manage` | refused | wait | wait |

`forge_read_file` and `forge_search_files` return file contents. On the
whole-node root (`root: node`) they could reveal any readable file on the
node, such as configuration with passwords or private keys, so such a call
waits for the person's approval in every mode, also `read_only`. On a named
file root they run like other reads. `forge_list_files` and
`forge_stat_file` show names and details only and always run.

No tool in the catalog wraps an action under the MFA guardrail today. The
rule for such tools is in [Modes](#modes). Approving a destructive tool
(`forge_deploy_stack`) still needs an MFA-verified session when the server
requires MFA, because its handler may check a guarded action from the
request, such as `stack.privileged`; see [Approvals](#approvals).

File tools also follow the file root's own rules. Agents must name an
approved folder or a container as `root`; the whole-node root (`node`) is
refused for every agent, administrators included, because it holds keys
and system secrets that must not reach a model provider. See
[File explorer](/docs/concepts/file-explorer/).

### Annotations and schemas

Every tool in `tools/list` has `name`, `title`, `description`, an
`inputSchema` (`type: object`, `additionalProperties: false`, with
`required`) and these annotations. When a tool waits for approval in the
connection's mode, its `description` ends with
`Waits for approval in the Forge console by the person who connected you.`

| Annotation | Value |
| --- | --- |
| `title` | The tool's title |
| `readOnlyHint` | `true` for read tools |
| `destructiveHint` | `true` for destructive tools |
| `idempotentHint` | `true` for every tool except `forge_deploy_stack` |
| `openWorldHint` | `false` |

### Arguments

All arguments are optional unless marked required. Id arguments (`node`,
`job`, `stack`, `service`, `approval`) are 1 to 128 characters of letters, digits,
`-` and `_`. Unknown arguments are refused.

| Tool | Argument | Type | Description |
| --- | --- | --- | --- |
| `forge_list_nodes` | | | No arguments |
| `forge_get_node` | `node` | string, required | Node id (from `forge_list_nodes`) |
| `forge_node_metrics` | `node` | string, required | Node id |
| `forge_list_clusters` | | | No arguments |
| `forge_list_environments` | | | No arguments |
| `forge_list_jobs` | `status` | string | `queued`, `running`, `succeeded`, `failed` or `cancelled` |
| | `limit` | integer | Most recent N (default 50) |
| `forge_get_job` | `job` | string, required | Job id |
| `forge_job_steps` | `job` | string, required | Job id |
| `forge_list_services` | `environment_id` | string | Environment id |
| | `cluster_id` | string | Cluster id |
| `forge_get_service` | `service` | string, required | Service id |
| `forge_list_stacks` | `environment_id` | string | Environment id |
| | `server_id` | string | Node id |
| `forge_get_stack` | `stack` | string, required | Stack id (from `forge_list_stacks`) |
| `forge_stack_revisions` | `stack` | string, required | Stack id |
| `forge_file_roots` | `node` | string, required | Node id |
| `forge_list_files` | `node` | string, required | Node id |
| | `root` | string, required | File root id from `forge_file_roots`: an approved folder or a container. The whole-node root is refused |
| | `path` | string | Absolute path inside the root, such as `/etc/nginx` |
| | `limit` | integer | Entries per page (default 1000) |
| | `offset` | integer | Skip this many entries |
| | `hidden` | boolean | Include hidden entries |
| `forge_stat_file` | `node` | string, required | Node id |
| | `root` | string, required | File root id from `forge_file_roots`: an approved folder or a container. The whole-node root is refused |
| | `path` | string | Absolute path |
| `forge_read_file` | `node` | string, required | Node id |
| | `root` | string, required | File root id from `forge_file_roots`: an approved folder or a container. The whole-node root is refused |
| | `path` | string | Absolute path |
| | `offset` | integer | Byte offset |
| | `length` | integer | Bytes to read (default and maximum 256 KiB) |
| `forge_search_files` | `node` | string, required | Node id |
| | `root` | string, required | File root id from `forge_file_roots`: an approved folder or a container. The whole-node root is refused |
| | `path` | string | Folder to search under |
| | `spec` | object, required | Search, such as `{"name": "*.conf", "content": "listen", "regex": false}` |
| `forge_audit_log` | `limit` | integer | Most recent N (default 100) |
| `forge_approval_status` | `approval` | string, required | Approval id (`ap_…`) returned by a tool that waits for approval |
| `forge_refresh_stack` | `stack` | string, required | Stack id |
| `forge_stack_service_action` | `stack` | string, required | Stack id |
| | `server_id` | string, required | Node id |
| | `action` | string, required | `start`, `stop`, `restart` or `pull` |
| | `services` | array of strings | Service names; empty: all |
| `forge_cancel_job` | `job` | string, required | Job id |
| `forge_deploy_stack` | `stack` | string, required | Stack id |
| | `revision` | integer | Revision; default: the latest |
| | `services` | array of strings | Services to start; empty: all |
| | `pull` | boolean | Pull images again |

Example call:

```json
{"jsonrpc": "2.0", "id": 2, "method": "tools/call",
 "params": {"name": "forge_list_files", "arguments": {"node": "<node-id>", "root": "<root-id>", "path": "/etc/nginx"}}}
```

## Modes

Set per connection on the consent page; capped by `[mcp] max_mode`. The
consent page's default is `ask`, or `max_mode` when that is lower.

| Mode | Value | Read tools | Write tools | Destructive tools | MFA-guarded tools |
| --- | --- | --- | --- | --- | --- |
| **Read-only** | `read_only` | Listed and run | Not listed; refused | Not listed; refused | Not listed; refused |
| **Ask** (default) | `ask` | Listed and run | Listed; wait for approval | Listed; wait for approval | Listed; wait for approval |
| **Auto** | `auto` | Listed and run | Listed and run | Listed; wait for approval | Listed; wait for approval |

An **MFA-guarded tool** is one whose REST endpoint's Cedar action is under
the `guard:sensitive-requires-mfa` guardrail, whatever its effect. A call
that runs directly and reaches an MFA-guarded check at run time, such as a
privileged stack, fails with the MFA message below. Connector sessions
never count as MFA-verified, also when the server does not require MFA
(`[security] require_mfa = false`, or the deprecated `dev_skip_mfa`).

Not available: mode caps per role or workspace.

## Approvals

In `ask` and `auto`, a call to a tool that waits for approval does not run.
Forge validates the arguments first; invalid arguments give the usual
[tool error](#tool-errors) and nothing is queued. Valid calls are stored as
a pending approval of kind `mcp.tool` for the person who owns the
connection, and the call returns a normal result, not an error:

| Field | Value |
| --- | --- |
| `content` | `[{"type": "text", "text": "Waiting for approval <id>: the person who connected you approves or denies it in the Forge console (<url>). It expires in 15 minutes. Check the outcome with forge_approval_status."}]` |
| `structuredContent.approval_id` | string, `ap_…` |
| `structuredContent.status` | `"pending"` |
| `structuredContent.url` | `<public_url>/approvals?id=<approval_id>`: the console page that opens the request |
| `structuredContent.expires_at` | integer, epoch ms: 15 minutes after the call |

```json
{"content": [{"type": "text", "text": "Waiting for approval ap_<random>: …"}],
 "structuredContent": {"approval_id": "ap_<random>", "status": "pending",
   "url": "https://forge.example.com/approvals?id=ap_<random>", "expires_at": 1767226500000}}
```

The agent reads the outcome with `forge_approval_status`, which returns the
[approval object](/docs/reference/rest-api/#approvals) of `GET /api/v1/approvals/{id}`.
A connector can read only approvals its own connection asked for; any
other id gives `not found`.

| `status` | Meaning | `result` / `error` |
| --- | --- | --- |
| `pending` | Waiting for the person | `null` |
| `approved` | Approved; running now | `null` |
| `executed` | Ran and succeeded | `result`: the REST response |
| `failed` | Ran and the handler failed or refused | `error`: the reason, as in [Tool errors](#tool-errors) |
| `denied` | The person denied it; nothing ran | `null` |
| `expired` | Nobody decided within 15 minutes; nothing ran | `null` |
| `cancelled` | The connection or its connector was revoked first; nothing ran | `null` |

Rules:

| Rule | Value |
| --- | --- |
| Who decides | Only the person who owns the connection. Approving needs a console (browser) session with its CSRF token; API keys, app and CLI bearer tokens and connector tokens cannot approve. Denying also works from the CLI (`forge approvals deny`) or an API key, never with a connector token |
| MFA | When the tool is destructive or MFA-guarded (`mfa_required: true`) and `[security] require_mfa` is on, approving needs an MFA-verified session; otherwise `403` `step_up_required` |
| Still allowed | Approving checks again that the connection is not revoked, its connector is not revoked, `[mcp] enabled` is `true`, and the connection's mode under the current `[mcp] max_mode` still allows the tool. Otherwise `409` and nothing runs; the request stays pending until it expires, unless it was cancelled |
| Revocation | Revoking a connection, or the connector it was made with, cancels its pending approvals (status `cancelled`) |
| Refusals | Every refused approve or deny (wrong person, wrong kind of session, missing MFA, no longer allowed, already decided) is audited as `mcp.approval.decide` with outcome denied and the reason |
| How it runs | Once, through the same `/api/v1` router, with the approver's own session cookie and CSRF token, never the connector's token. Cedar and the MFA guardrail are checked against the approver at that moment |
| Use | Single use: the first approve or deny wins; later ones get `409` |
| Lifetime | 15 minutes from the call |
| Pending per connection | At most 10 unexpired. Expired requests do not count; they are stored as `expired` when the next request is queued |
| Storage | Table `approvals` (migration `0014`); survives a restart. Decided, expired and cancelled approvals are deleted 90 days after they were created; the audit log keeps the record |

For the console and REST side, see
[REST API: Approvals](/docs/reference/rest-api/#approvals) and
[Approve what an agent asks for](/docs/guides/connect-ai-agents/#approve-what-an-agent-asks-for).

## Tool results

A successful call returns:

| Field | Value |
| --- | --- |
| `content` | `[{"type": "text", "text": "<JSON of the REST response>"}]` |
| `structuredContent` | The REST response, when it is a JSON object and the result was not cut |

Fields ending in `_b64` whose value decodes to UTF-8 text are replaced by
`_text` fields (for example `content_b64` becomes `content_text`). Results
over 64 KiB are cut and end with
`[result cut at 64 KiB: narrow the request (limit, path, offset)]`.

## Limits

| Limit | Value | When exceeded |
| --- | --- | --- |
| Request body | 1 MiB | `413`, JSON-RPC error `-32600` `request too large` |
| Tool calls per connection | 60 per minute, burst 20 | Tool error `too many tool calls; wait a moment` |
| Tool calls at once per connection | 4 | Tool error `too many tool calls at once for this connection` |
| Tool result | 64 KiB | Cut, with a note |
| Error detail passed to the agent | 500 characters | Cut |
| `forge_read_file` length | 256 KiB | Limited by the files API |
| Pending consent requests | 1,000, each valid 10 minutes, kept in memory | `429 throttled`; a restart drops them |
| Dynamic client registrations | `registrations_per_hour` (default 30), across all callers | `429 throttled` |
| Pending approvals per connection | 10 | Tool error `too many requests are waiting for approval; ask the person to decide them first` |
| Approval lifetime | 15 minutes | Status `expired`; approving gives `409` |
| Approved request's result | 1 MiB | Status `failed`, `error`: `the result is too large` |

Rate and concurrency limits count calls that run; a call queued for
approval counts toward the pending cap instead. They are kept in memory per
server process.
Approvals are stored in the database.

## Errors

### HTTP and JSON-RPC errors

| Status | Body | Cause |
| --- | --- | --- |
| `401` | `{"error": "invalid_token"}`, `WWW-Authenticate: Bearer resource_metadata="<public_url>/.well-known/oauth-protected-resource/mcp"` (with `error="invalid_token"` when a bearer token other than an API key was refused) | No token, an API key (`fk_…`), an unknown or expired token, a revoked connection, or a token that is not a connector's for this resource |
| `403` | `{"error": "origin not allowed"}` | `Origin` is not the origin of `public_url` |
| `404` | `{"error": "MCP is not enabled on this Forge"}` | `[mcp] enabled = false` |
| `405` | empty, `Allow: POST` | `GET` or `DELETE` |
| `400` | JSON-RPC `-32600` `unsupported MCP-Protocol-Version` | Header names an unsupported version |
| `413` | JSON-RPC `-32600` `request too large` | Body over 1 MiB |
| `400` | JSON-RPC `-32700` `parse error` | Body is not JSON |
| `400` | JSON-RPC `-32600` `send one JSON-RPC message (no batches)` | Body is an array or not an object |
| `503` | `{"error": "try again"}` | The token could not be checked (database unavailable) |
| `200` | JSON-RPC `-32601` `method not found` | Unknown method |
| `200` | JSON-RPC `-32602` ``unknown tool `<name>` `` | `tools/call` with an unknown tool name |

### Tool errors

Other failures return a normal result with `isError: true` and one text
item, so the agent can read and act on them:

| Text | Cause |
| --- | --- |
| `<tool>: this connection is read-only` | A write, destructive or MFA-guarded tool on a **Read-only** connection |
| `too many requests are waiting for approval; ask the person to decide them first` | 10 unexpired approvals from this connection are pending |
| `this action needs multi-factor authentication; a person must do it in the Forge console` | The handler refused with `step_up_required` or `mfa_required` |
| `not allowed: <detail>` | `403` from the handler (Cedar denied) |
| `not found: <detail>` | `404` from the handler |
| `failed (<status>): <detail>` | Any other error from the handler |
| ``unknown argument `<name>` `` | An argument the tool does not have |
| ``missing argument `<name>` `` | A required argument is missing or `null` |
| `` `<name>` must be a <type> `` | Wrong JSON type |
| `` `<name>` is not a valid id `` | An id with other characters, or over 128 |
| `arguments must be an object` | `arguments` is not an object |
| `agents cannot use the whole-node root: pick an approved folder or a container from forge_file_roots` | `root` is empty or `node` |
| `too many tool calls; wait a moment` | Rate limit |
| `too many tool calls at once for this connection` | Concurrency limit |

## Configuration

`[mcp]` in `forge.toml`. See [Configuration](/docs/reference/configuration/#mcp-connectors).

| Key | Type | Default | Env | Description |
| --- | --- | --- | --- | --- |
| `mcp.enabled` | boolean | `false` | `FORGE_MCP_ENABLED` (`1` or `true`) | Serve `/mcp`, the protected resource metadata, registration and consent. Listing and revoking connections work either way |
| `mcp.max_mode` | string | `"auto"` | `FORGE_MCP_MAX_MODE` | Highest mode a person may give: `read_only`, `ask` or `auto`. Also caps the consent page's default (`ask`). Other values stop Forge from starting |
| `mcp.registrations_per_hour` | integer | `30` | none | Dynamic client registrations accepted per rolling hour, across all callers |

`public_url` sets the issuer, the resource identifier `<public_url>/mcp`
and the allowed `Origin`.

## OAuth endpoints

These sit at the root of the host, outside `/api/v1`. For the consent and
Connected apps APIs, see [REST API: MCP connectors](/docs/reference/rest-api/#mcp-connectors).

| Method | Path | Standard | Notes |
| --- | --- | --- | --- |
| GET | `/.well-known/oauth-authorization-server` | RFC 8414 | Always served; `registration_endpoint` only when MCP is enabled |
| GET | `/.well-known/oauth-protected-resource` | RFC 9728 | `404` when MCP is off |
| GET | `/.well-known/oauth-protected-resource/mcp` | RFC 9728 | Same document; the URL `WWW-Authenticate` names |
| POST | `/oauth/register` | RFC 7591 | Anonymous, capped per hour. `404` when MCP is off |
| GET | `/oauth/authorize` | OAuth 2.1 + PKCE, RFC 8707 | Registered connectors go to the consent page `/connect?request=<id>` |
| POST | `/oauth/token` | OAuth 2.1, RFC 8707 | Form-encoded; `authorization_code` and `refresh_token` grants |

### Authorization server metadata

| Field | Value |
| --- | --- |
| `issuer` | `public_url` without a trailing slash |
| `authorization_endpoint` | `<issuer>/oauth/authorize` |
| `token_endpoint` | `<issuer>/oauth/token` |
| `registration_endpoint` | `<issuer>/oauth/register` (MCP enabled only) |
| `response_types_supported` | `["code"]` |
| `response_modes_supported` | `["query"]` |
| `grant_types_supported` | `["authorization_code", "refresh_token"]` |
| `code_challenge_methods_supported` | `["S256"]` |
| `token_endpoint_auth_methods_supported` | `["none"]` |
| `authorization_response_iss_parameter_supported` | `true` |

### Protected resource metadata

| Field | Value |
| --- | --- |
| `resource` | `<issuer>/mcp` |
| `authorization_servers` | `["<issuer>"]` |
| `bearer_methods_supported` | `["header"]` |
| `resource_name` | `Orbyn Forge` |
| `resource_documentation` | `https://docs.orbyn.dev/concepts/mcp-connectors/` |

### Registration (`POST /oauth/register`)

JSON request. Unknown fields are ignored.

| Field | Type | Rule |
| --- | --- | --- |
| `client_name` | string | Default `Unnamed connector`. Control characters removed; 1–80 characters after trimming |
| `redirect_uris` | array of strings | Required, 1–10, each following the [redirect URI rules](#redirect-uri-rules) |
| `grant_types` | array of strings | Optional; only `authorization_code` and `refresh_token` |
| `response_types` | array of strings | Optional; only `code` |
| `token_endpoint_auth_method` | string | Optional; only `none` (public clients) |

`201 Created` response:

```json
{
  "client_id": "mcp_<random>",
  "client_id_issued_at": 1767225600,
  "client_name": "Claude",
  "redirect_uris": ["https://claude.ai/api/mcp/auth_callback"],
  "grant_types": ["authorization_code", "refresh_token"],
  "response_types": ["code"],
  "token_endpoint_auth_method": "none"
}
```

`client_id_issued_at` is in epoch seconds. No client secret is issued.

| Status | Body | Cause |
| --- | --- | --- |
| `400` | `{"error": "invalid_client_metadata", "error_description": "…"}` | Bad name, grant type, response type or auth method |
| `400` | `{"error": "invalid_redirect_uri", "error_description": "…"}` | A redirect URI breaks the rules, or none or more than 10 |
| `429` | problem details, `title: throttled` | `registrations_per_hour` reached |
| `404` | problem details | MCP is off |

### Redirect URI rules

A redirect URI is accepted when it is at most 2,048 characters, contains no
`*`, `#` or whitespace, parses as a URL, has no user info, and is one of:

| Kind | Rule | Example |
| --- | --- | --- |
| HTTPS | `https` with a host | `https://claude.ai/api/mcp/auth_callback` |
| Loopback | `http` on `127.0.0.1`, `[::1]` or `localhost` | `http://127.0.0.1:33418/callback` |
| App scheme | A private-use scheme for native apps | `cursor://anysphere.cursor-retrieval/oauth/callback` |

Refused schemes: `javascript`, `data`, `file`, `vbscript`, `about`, `blob`,
`ftp`, `ws`, `wss`, and `http` on any other host.

In an authorization request, `redirect_uri` must equal a registered URI
exactly, including the port.

### Authorize and token

| Parameter | Rule |
| --- | --- |
| `response_type` | `code` |
| `client_id` | A registered, not revoked connector |
| `redirect_uri` | Exactly one of the client's registered URIs |
| `code_challenge`, `code_challenge_method` | PKCE, `S256`, a 43-character challenge |
| `state` | Optional; returned unchanged |
| `resource` | Optional; if present, must be `<issuer>/mcp`, else `400 invalid_target` |

Without a web session, `/oauth/authorize` sends the browser to `/login` and
back. Errors are shown in the browser, never redirected to the client.
After **Approve**, the browser goes to `redirect_uri` with `code`, `state`
and `iss`. After **Deny**, it gets `error=access_denied`, `state` and `iss`.

| Item | Value |
| --- | --- |
| Authorization code | Single use, valid 60 seconds |
| Access token | Bearer, valid 1 hour, bound to the connection and to `<issuer>/mcp` |
| Refresh token | Rotates on every use; valid 30 days from issue |
| `resource` at `/oauth/token` | Optional; if present, must be `<issuer>/mcp`, else `400 invalid_target` |
| Revoked connection | Code exchange and refresh fail with `invalid_grant` (`the connection was revoked`) |
| MFA | Connector tokens are never MFA-verified |

The first-party client `forge-app` (the desktop and mobile apps) may not
send `resource`; it gets `400 invalid_target`.

### Where tokens are accepted

| Token | At `/mcp` | At `/api/v1` |
| --- | --- | --- |
| Connector access token | Accepted | `401 invalid_token`, `this token is for the MCP endpoint only` |
| API key (`fk_…`) | `401` | Accepted |
| Desktop or mobile app bearer token | `401` | Accepted |
| Browser session cookie | `401` (cookies are not read) | Accepted |

## Audit

| Action | Actor | Details | When |
| --- | --- | --- | --- |
| `oauth.client.register` | `connector:anonymous` | `client_id`, `client_name`, `redirect_hosts` | A client registers |
| `connector.approve` | The person | `connection`, `client_id`, `client_name`, `redirect_host`, `mode` | **Approve** on the consent page |
| `connector.approve` (denied) | The person | `reason: "<name> was not approved"` | **Deny** on the consent page |
| `connector.revoke` | The person who revoked | `connection`, `client_name`, `owner` | A connection is revoked |
| `oauth.client.revoke` | The administrator | `client_id`, `client_name` | A registered connector is revoked |
| `mcp.approval.request` | The person who owns the connection | `approval`, `tool`, `summary`, `via` (`connector`, `client_id`, `connection`, `mode`) | A tool call is queued for approval |
| `mcp.approval.decide` | The person who decided | `approval`, `action` (the tool), `approved: true`, `requester` (connector name) | **Approve and run** |
| `mcp.approval.decide` (denied) | The person who decided | `reason: "<tool> denied: <approval id>"` | **Deny** |
| `mcp.approval.decide` (denied) | The person who tried | `reason: "<approval id> (<tool>): <why>"` | An approve or deny was refused: wrong person or session, `step_up_required`, connection or connector revoked, MCP turned off, mode no longer allows it, or already decided or expired |
| `mcp.approval.cancel` | The person who revoked | `approval`, `connection`, `client_name`, `reason: "connection revoked"` | A pending approval is cancelled because its connection or connector was revoked |
| `mcp.tool.denied` (denied) | The person who owns the connection | `tool`, `reason`, `via` | Forge refused a call before it reached a handler: a read-only connection, the rate or concurrency limit, or too many pending approvals |

Every event the API records during a connector's tool call, including
denials, keeps the person as the actor and adds:

```json
"via": {"connector": "Claude", "client_id": "mcp_<random>", "connection": "ac_<random>", "mode": "auto"}
```

When an approved request runs, the handler's own events keep the approving
person as the actor and add:

```json
"via": {"approval": "ap_<random>", "connector": "Claude"}
```
Reads are not audited, as in the console.
