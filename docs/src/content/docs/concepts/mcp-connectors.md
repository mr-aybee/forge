---
title: MCP connectors
description: How AI agents such as Claude, ChatGPT, Gemini, Codex and Cursor connect to Forge over the Model Context Protocol, how they sign in, and what the server lets them do.
sidebar:
  order: 12
---

Forge serves the Model Context Protocol (MCP) itself, at `/mcp` on the
control plane. An AI agent connects to it as a **connector**: a person pastes
the Forge URL into the agent, signs in to Forge in the browser and chooses
how much the agent may do. This page explains the sign-in flow, the modes,
approvals, and how tool calls reach your nodes. To connect an agent, see
[Connect AI agents](/docs/guides/connect-ai-agents/). For every method, tool and
field, see the [MCP reference](/docs/reference/mcp/).

MCP connectors are off by default. An administrator turns them on with
`[mcp] enabled = true` (see [Configuration](/docs/reference/configuration/#mcp-connectors)).

## What a connector is

A connector is an OAuth client that registered itself with Forge, plus one
or more **connections**: each connection is one approval a person gave on
the consent page. The connection records who approved it, the client, the
host the browser returned to, and the **mode**. Every token the connector
holds points to its connection, and the token works only while the
connection exists.

The agent runs wherever its vendor runs it: in a cloud service (claude.ai,
ChatGPT), on a desktop (Claude Desktop), or in a terminal or editor (Claude
Code, Gemini CLI, Codex CLI, Cursor). In every case it talks to the same
endpoint and acts as the person who approved it.

## How sign-in works

Agents find out how to sign in by themselves. A person only gives them the
URL `https://<forge-host>/mcp`.

```diagram
alt: MCP connector OAuth sign-in sequence
sequence:
  actors: [Agent, Forge, Person (browser)]
  messages:
    - { from: Agent, to: Forge, label: "POST /mcp (no token)" }
    - { from: Forge, to: Agent, label: "401 + WWW-Authenticate: resource_metadata=…" }
    - { from: Agent, to: Forge, label: "GET /.well-known/oauth-protected-resource/mcp (RFC 9728)" }
    - { from: Agent, to: Forge, label: "GET /.well-known/oauth-authorization-server (RFC 8414)" }
    - { from: Agent, to: Forge, label: "POST /oauth/register (RFC 7591)" }
    - { from: Forge, to: Agent, label: client_id }
    - { from: Agent, to: Person (browser), label: "open /oauth/authorize?…&resource=…/mcp (PKCE S256, RFC 8707)" }
    - { at: Forge, span: Person (browser), note: "sign in (password, MFA, passkey, SSO) → /connect consent page" }
    - { from: Person (browser), to: Forge, label: Approve + mode }
    - { from: Person (browser), to: Agent, label: redirect with code }
    - { from: Agent, to: Forge, label: "POST /oauth/token (code + verifier)" }
    - { from: Forge, to: Agent, label: "access + refresh token, bound to /mcp" }
    - { from: Agent, to: Forge, label: "POST /mcp  Authorization: Bearer …" }
```

1. **Discovery.** The first call without a token gets `401` with a
   `WWW-Authenticate` header that names the protected resource metadata. That
   document names Forge as the authorization server, and Forge's
   authorization server metadata names the authorize, token and registration
   endpoints.
2. **Registration.** The agent registers itself with dynamic client
   registration and gets a `client_id`. Registration is anonymous, so it
   grants nothing: it only lets the client ask a person. Forge accepts public
   clients only (no client secret), checks every redirect URI, and caps
   registrations per hour across all callers.
3. **Consent.** The agent opens Forge's authorize endpoint in the browser.
   The person signs in the usual way, with MFA, passkeys or SSO, and Forge
   shows the consent page: the name the app gave itself, where the browser
   goes next, and the modes. Any app can choose any name, so the page shows
   the redirect host too. Consent needs the person's browser session; a
   token or API key cannot approve a connector, so an agent cannot approve
   itself.
4. **Token.** After **Approve**, the browser returns to the agent with a
   one-time code. The agent exchanges it, with its PKCE verifier, for an
   access token and a refresh token. Both are bound to the connection and to
   the resource `https://<forge-host>/mcp` (RFC 8707 resource indicators).

### Audience-bound tokens

A connector's token is accepted at `/mcp` and nowhere else. The rest of the
API answers `401` with `this token is for the MCP endpoint only`. The other
way round, `/mcp` accepts only connector tokens: API keys, desktop and
mobile app tokens and browser sessions get `401`. A token that
leaks from an agent therefore cannot drive the console API, and a script's
API key cannot pose as an agent.

Revoking a connection in **Profile** → **Connected apps** ends its tokens at
once, including refresh tokens, and cancels the requests it left waiting
for approval. Revoking a registered connector ends every connection made
with it, and cancels their waiting requests too.

## Modes

The person picks the mode on the consent page. Forge enforces it on every
call; the agent cannot change it.

| Mode | Reads | Reversible changes | Destructive and MFA-guarded changes |
| --- | --- | --- | --- |
| **Read-only** | Run | Refused | Refused |
| **Ask** (default) | Run | Wait for approval | Wait for approval |
| **Auto** | Run | Run | Wait for approval |

- **Reads** list and read nodes, metrics, jobs, stacks, files and the audit
  log.
- **Reversible changes** refresh a stack, start, stop, restart or pull stack
  services, and cancel a job.
- **Destructive changes** replace running workloads, such as deploying a
  stack revision.
- **MFA-guarded changes** are tools whose Cedar action is under the MFA
  guardrail, such as running commands, writing or deleting files,
  privileged stacks and network changes. The
  [Cedar schema reference](/docs/reference/cedar-schema/#actions) marks every
  action the guardrail covers.

**Ask** is the default on the consent page. It lets the agent look freely
and propose changes, while a person decides each one. **Auto** suits
routine, reversible work with an agent you trust; anything destructive or
MFA-guarded still waits for a person. In **Read-only**, the agent sees only
the tools it can run.

An administrator caps the highest mode for the whole server with
`[mcp] max_mode`. With `max_mode = "read_only"`, the consent page offers
**Read-only** only and it becomes the default.

## How approvals work

In **Ask** and **Auto**, a change that needs a person does not run when
the agent calls it. Forge checks the arguments, stores the request as a
pending **approval** for the person who connected the agent, and answers
the agent with the approval's id, a link to the **Approvals** page and when
it expires. The agent tells the person and can follow the outcome with the
`forge_approval_status` tool.

```diagram
alt: MCP approval sequence
sequence:
  actors: [Agent, Forge, Person (console)]
  messages:
    - { from: Agent, to: Forge, label: "tools/call forge_deploy_stack" }
    - { at: Forge, note: "check arguments, store approval (audit: `mcp.approval.request`)" }
    - { from: Forge, to: Agent, label: "{approval_id, status: pending, url, expires_at}" }
    - { from: Person (console), to: Forge, label: Approve and run }
    - { at: Forge, note: "run the stored request once as the person (Cedar, MFA, audit)" }
    - { from: Agent, to: Forge, label: forge_approval_status }
    - { from: Forge, to: Agent, label: "status: executed + result" }
```

An approval is single use. It expires 15 minutes after the agent asked, and
at most 10 can wait per connection, so an agent cannot bury a person in
requests. Only the person whose connection asked can approve or deny it.
Approving happens only in the console: an agent that runs shell commands may
share the person's CLI session, so a CLI approval would let it approve its
own request. Denying is safe anywhere, so `forge approvals deny` works too.

### Why approved changes run as the person

When the person approves, Forge runs the stored request once, through the
same `/api/v1` router the console uses, with the approver's own console
session. It does not use the agent's token. So:

- **The agent cannot approve.** Approving needs a browser session with its
  CSRF token. Connector tokens, API keys and app tokens are refused, and a
  connector can only read the status of its own requests.
- **MFA stays with people.** When the change is destructive or its action
  is under the MFA guardrail, and the server requires MFA, approving needs
  an MFA-verified session. Destructive tools count because their handler may
  check a guarded action that the request alone does not show, such as a
  privileged stack. The
  person verifies, as for any guarded action in the console; the agent
  never holds MFA.
- **Permissions are checked at run time.** The handler checks Cedar against
  the approver when the change runs, not when the agent asked. If the
  person lost the permission in between, the change fails.
- **The connection must still allow it.** A request waits up to 15
  minutes, and the situation can change meanwhile. Approving checks again
  that the connection and its connector are not revoked, that connectors are
  still turned on, and that the connection's mode, under the current
  `[mcp] max_mode`, still allows the change. If not, nothing runs.
- **The audit trail names both.** The handler's own audit entry has the
  person as actor and a `via` field with the approval id and the
  connector's name. Refused decisions are audited too, with the reason.

### Check the request, not the conversation

The request on the **Approvals** page came from an AI agent. An agent that
read a poisoned log line or file can ask for something the person never
meant. Forge shows the exact tool and arguments that will run, as data.
Approve what the request says, not what the agent said it would do.

### Why modes are enforced by the server

Agents have their own permission settings, such as "ask before changes".
Those help the person at the keyboard, but the agent's vendor enforces them,
not Forge, and a prompt-injected or misconfigured agent can ignore them.
Forge's mode is checked on the server for each call, so it holds whatever
the agent does.

### Why agents never hold MFA

A person's web session can be MFA-verified after step-up; a connector's
session never is. Step-up proves that a person is present, and an agent
acting on its own cannot prove that. Forge therefore issues connector
tokens without MFA and clears the MFA flag on every connector call. This
holds on a server that does not require MFA (`[security] require_mfa =
false`) too: there, people's sessions count as MFA-verified, but a
connector's never does. Every action under the MFA guardrail fails for an
agent, whatever the mode, whatever role the person holds and whatever the
server's MFA setting. The MCP endpoint also checks the guardrail list itself:
it never runs a tool whose Cedar action is on it, nor any destructive tool,
with the connector's token. Such a call waits for approval, and runs only
as the person who approves it.

## How tool calls run

Each tool is a thin wrapper around one REST endpoint. When an agent calls a
tool, Forge builds the REST request from the tool's arguments and runs it
in process through the same `/api/v1` router the console uses. The request
carries only the connector's own token and an in-process marker that HTTP
clients cannot set. It does not copy headers, cookies or anything else from
the agent's request.

```diagram
alt: How an MCP tool call becomes an API call
flow:
  direction: column
  steps:
    - { label: "POST /mcp tools/call forge_list_files {node, root, path}", edge: "mode check (run, wait for approval, or refuse) · argument check · rate limit" }
    - { label: "GET /api/v1/servers/{id}/files/list?root=…&path=…", note: in process, connector token, tone: accent, edge: "same handler · Cedar authorization · MFA guardrail · audit" }
    - { label: JSON result → MCP content, note: cut at 64 KiB }
```

So an agent gets exactly the person's permissions, never more. A tool call
passes the same [Cedar authorization](/docs/concepts/authorization/), the same
MFA guardrail and the same [audit](/docs/concepts/audit/) as the console. There
is no other path from `/mcp` to data.

The audit log records the person as the actor. Events from a connector's
calls also carry a `via` field with the connector's name, client id,
connection id and mode. As in the console, reads are not audited; changes
and denials are.

## Tool results are data

Tool results can contain text that someone else controls: file contents,
logs, host names, container labels. An attacker can write instructions into
a log line and hope an agent follows them. Forge reduces the effect:

- Forge tells the agent, when it connects, that tool results are data from
  managed systems, never instructions. Tools that return file contents say
  so in their description.
- Consent and the mode live in the console, never in the conversation. An
  agent cannot raise its own mode or approve a connection.
- Destructive and MFA-guarded actions wait for a person to approve them in
  the console, and in **Ask** mode every change does. The **Approvals** page
  shows the request as data from an AI agent.
- Read-only connections cannot change anything, whatever the agent is told.
- File tools work only in approved folders and containers. The whole-node
  file root, which holds keys and system secrets, is refused for every
  agent, administrators included, so its contents never reach a model
  provider.

Forge cannot stop an agent from repeating what it read, or from acting on
it within its mode. Give agents the lowest mode that does the job, prefer
**Ask** when an agent should change things, and use **Auto** only with
agents and people you trust with reversible changes.

## Reachability

The agent, not your browser, calls `/mcp`, so the agent must reach Forge:

| Agent | Where it runs | What it needs |
| --- | --- | --- |
| claude.ai, ChatGPT | The vendor's cloud | Forge reachable from the internet over HTTPS at its `public_url` |
| Claude Desktop custom connectors | The vendor's cloud (the desktop app uses the same connectors as claude.ai) | Same as claude.ai |
| Claude Code, Gemini CLI, Codex CLI, Cursor | Your machine | Forge reachable from that machine, for example over a VPN or office network |

A private or air-gapped Forge works with agents that run on a machine that
can reach it, and with the [`forge` CLI](/docs/reference/cli/). Cloud connectors
cannot reach it.

Forge builds the metadata, the resource identifier and the `Origin` check
from `public_url`. Set it to the exact HTTPS URL agents use, and serve Forge
at the root of that host name.

## Limits

Forge protects itself and the nodes from runaway agents:

| Limit | Value |
| --- | --- |
| Tool calls per connection | 60 per minute, bursts of 20 |
| Tool calls running at once per connection | 4 |
| Tool result size | 64 KiB; longer results are cut with a note to narrow the request |
| Request size | 1 MiB |
| Dynamic client registrations | `[mcp] registrations_per_hour` (default 30), across all callers |
| Consent page | 10 minutes, then the agent must start again |
| Approvals waiting per connection | 10; the agent must wait until the person decides some |
| Approval lifetime | 15 minutes, then it expires and cannot run |

Rate limits are counted in memory per server process. Approvals are stored
in the database and survive a restart.

## What is not supported yet

- **Approval notifications.** Forge does not notify the person of a new
  approval by email or chat. The agent passes on the link, and the
  **Approvals** page lists what waits.
- **Resources and prompts.** `resources/list`, `resources/templates/list`
  and `prompts/list` return empty lists.
- **Streaming.** No SSE stream, no progress notifications and no
  cancellation over MCP. Each call returns one JSON response.
- **Mode caps per role or workspace.** Only the server-wide `max_mode`.
- **Generated tools.** The tool catalog is curated by hand; it does not yet
  cover every REST endpoint or use typed OpenAPI schemas.
- **A client allowlist.** Any client may register itself; there is no mode
  that accepts only pre-registered clients. Administrators review and revoke registered connectors
  in the **Registered connectors** panel under **Profile**.
