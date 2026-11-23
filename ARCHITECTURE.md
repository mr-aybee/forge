# Orbyn Forge — Architecture

Orbyn Forge is an orchestration layer. It drives proven engines (Docker, Nginx,
Caddy, PowerDNS, guacd, database servers) and never reimplements them.
This document covers the core foundation, the features built on it and the
extension points that later phases plug into. The full user-facing docs live
in [`docs/`](docs/) (Astro Starlight).

## 1. Components

| Process | Crate | Responsibility | Trust |
| --- | --- | --- | --- |
| Control plane | `forge-server` | REST API + WebSocket, MCP endpoint (`/mcp`), identity, authorization, registry, jobs, audit, agent hub, plugin host | Highest: holds fleet state |
| Analytics worker | `forge-analytics-worker` | All analytics work (D-141, D-142): the mTLS spool upload listener for agents (`:55132`), import of spool files, ingest of off-fleet service logs and of old agents' heartbeat telemetry, node-minute and container buffers, the log batch writer, log searches and history reads, retention and runtime tuning, and the embedded ClickHouse engine as its own child, or the DuckDB store in its own process (D-143). A child of `forge-server`, which supervises it (restart on exit, hang or memory limit) and talks to it over a private Unix socket | Trusted; holds no control-plane state, never reads the operational database |
| Plugin runner | `forge-plugin-runner` | Executes one Wasm component under fuel/epoch/memory budgets, spawned on demand | Untrusted code |
| Node agent | `forge-agent` | Dials out to the control plane over mTLS, verifies signed commands, reports facts. Keeps the node's analytics spool, runs the local log listener (`127.0.0.1:5341` and the Docker bridge gateway) and uploads the spool to the analytics worker (D-142) | Trusted for its own server only |
| Privileged helper | `forge-agent-helper` | Runs the few privileged operations from a fixed allowlist | Root/SYSTEM, minimal surface |
| CLI | `forge-cli` (binary `forge`) | Operator and pipeline client of `/api/v1`: contexts, browser and device sign-in, step-up, typed commands, `forge api` from the OpenAPI document; tokens in the OS credential store | User credentials (bearer tokens) |
| Fleet simulator | `src/tools/fleet-sim` | Simulates 10–5,000 agents for benchmarks | Test only |

Library crates:

| Crate | Role |
| --- | --- |
| `forge-core` | Domain types, typed ids, access vocabulary, **ports** (traits) |
| `forge-authz` | Cedar schema, role templates, guardrails, `CedarAuthorizer` |
| `forge-store` | `sqlx` repositories + migrations (SQLite default, PostgreSQL feature) |
| `forge-s3` | S3 client adapter (SigV4 via `ring`, streamed bodies) for object store buckets and objects |
| `forge-jobs` | Durable job engine: leases, fencing tokens, idempotency, step state, retry with backoff |
| `forge-audit` | Append-only, hash-chained, signed audit log + verifier |
| `forge-health` | Observations: validation, freshness on the server clock, change detection and the ordered transition feed (D-085) |
| `forge-file-gateway` | Forge-managed SFTP gateway (`russh` + `russh-sftp`): temporary access grants, SFTP only, every request through `forge-files` |
| `forge-files` | File explorer service: roots, Cedar checks per operation, read-only roots, protected paths, upload ownership, audit (used by the API and the SFTP gateway) |
| `forge-rules` | Pure rule evaluation (sustained, window average, repeated, any-in-window, immediate; hysteresis recovery; unknown is not evidence), shared by alerts, automation and node alarms |
| `forge-alerts` | Alert evaluator: rules over evidence open and close durable alarm episodes, per workspace and scope. `notify`: notification channels and policies, the delivery planner and dispatcher, target rules and the payload (D-110) |
| `forge-pki` | Internal CA, CSR signing, enrollment tokens, rotation, revocation |
| `forge-proto` | Agent protocol (protobuf via `prost`), command signing and replay protection |
| `forge-analytics-proto` | Control protocol between `forge-server` and the analytics worker: framed typed messages, the worker configuration, and the analytics settings contract (D-141) |
| `forge-analytics` | ClickHouse adapter of the `AnalyticsStore` port (embedded engine or a ClickHouse server) |
| `forge-analytics-duckdb` | DuckDB adapter of `AnalyticsStore` and `SpoolFileWriter`: one DuckDB file per UTC day and family, one writer thread, hourly rollups, retention and compaction (D-143). Linked only into the worker |
| `forge-secrets` | Envelope encryption (AES-256-GCM via `ring`), pluggable key providers |
| `forge-identity` | Passwords (Argon2id), TOTP, sessions, CSRF, OAuth 2.1 + PKCE, OIDC SSO |
| `forge-plugin-host` | Manifest, signatures, lifecycle, permissions, quarantine, runner supervision |
| `forge-sdk` | WIT interfaces and Rust SDK for plugin authors |
| `forge-catalog` | App Store catalog (manifests, typed parameters, recipes) and service specs shared by the control plane and the root helper |
| `forge-ssh` | SSH adapter (`russh`): pinned host keys, node bootstrap, browser terminal sessions |

### Dependency direction (SOLID / hexagonal)

```
             forge-core  (types + ports, no I/O)
        ┌────────┬────────┬─────────┬──────────┬──────────┐
   forge-authz forge-jobs forge-audit forge-identity forge-plugin-host   ← services depend on ports only
        └────────┴────────┴─────────┴──────────┴──────────┘
                           ▲ implemented by
                      forge-store (adapters)
                           ▲ wired together in
                      forge-server (composition root)
```

Services never name a concrete adapter. `forge-server::state` is the only place
where concrete types are chosen and injected, so tests swap in fakes and later
phases add adapters (PostgreSQL, OpenBao, KMS) without touching services.

## 2. Trust boundaries

```
 ┌─ Browser / Tauri app ──────────┐ ┌─ forge CLI ───────────┐   ┌─ AI agent (MCP) ──┐
 │ React UI · HttpOnly cookie +   │ │ Bearer (PKCE loopback │   │ Bearer, aud=/mcp  │
 │ CSRF  |  app: Bearer (PKCE)    │ │ or device grant)      │   │ (connector token) │
 └───────────────┬────────────────┘ └──────────┬────────────┘   └─────────┬─────────┘
                 │ HTTPS /api/v1, /oauth, …    │                          │ POST /mcp
 ┌───────────────▼─────────────────────────────▼─ Control plane ──┐       │
 │ authn (login challenge, MFA policy) → Cedar authz → service    │◄──────┘
 │   → audit                                                      │
 │ job engine (re-authorizes every step)                          │
 │ agent hub (mTLS :7443)    plugin host ──spawn──┐               │
 │ analytics supervisor ──spawn──┐                │               │
 └──────────▲───────────────────│────────────────│───────────────┘
            │ outbound-only     │ Unix socket    │ stdio (JSON lines)
            │ HTTP/2 + protobuf │ 0600 + secret  ▼
            │ mTLS, signed cmds ▼             ┌─ Plugin runner ──────────┐
            │      ┌─ Analytics worker ─────┐ │ wasmtime, no WASI, typed │
            │      │ spool uploads (mTLS    │ │ host calls via host only │
            │      │ :55132), ingest, query │ └──────────────────────────┘
            │      │ └─ embedded engine     │
            │      │    (127.0.0.1 only)    │
            │      └──────────▲─────────────┘
            │                 │ outbound-only HTTP/2, mTLS
            │                 │ sealed spool files
 ┌──────────┴─────────────────┴─ Managed server ──┐
 │ forge-agent (unprivileged) · spool on disk     │
 │   ▲ local log listener :5341                   │
 │   │ (127.0.0.1 + Docker bridge gateway only)   │
 │   └ apps on this node (X-Seq-ApiKey)           │
 │ forge-agent-helper (allowlist)                 │
 │ engines: Caddy/Docker/... localhost            │
 └────────────────────────────────────────────────┘
```

1. **Browser ↔ control plane.** Credentials never reach the browser beyond an
   HttpOnly session cookie; mutating requests need a CSRF token. Apps use OAuth
   + PKCE and bearer tokens. No secrets in URLs or logs. Password sign-in
   carries a self-hosted proof-of-work challenge and is throttled per name and
   per client address (`X-Forwarded-For` is trusted only from
   `[security] trusted_proxies`). The console may also be published on a
   domain through a cluster ingress edge that terminates TLS and proxies
   plain HTTP (D-120): browser origins on WebSockets and `/mcp` are only
   `public_url` and the runtime public address (never the `Host` header);
   cookies get `Secure` from `cookie_secure` or from `X-Forwarded-Proto:
   https` sent by a trusted proxy. Links Forge sends people use the public
   address.
2. **CLI ↔ control plane.** The `forge` CLI is a first-party OAuth client
   (`forge-app`) and holds only bearer tokens, kept in the OS credential
   store (or an owner-only file where there is none). It never sees a
   password: sign-in happens in a browser, through a loopback redirect
   (RFC 8252) or the device grant (RFC 8628) approved by a person's console
   session. MFA-guarded calls answer `403 step_up_required` with
   `WWW-Authenticate: Bearer error="insufficient_user_authentication"`
   (RFC 9470); the CLI posts a TOTP code to `/auth/step-up` and gets a new
   token pair that replaces the session. API keys (`FORGE_TOKEN`) cannot
   step up. Servers must be `https://` except on loopback (or with
   `FORGE_ALLOW_HTTP`, for labs).
3. **AI agent ↔ control plane (MCP).** Agents call `POST /mcp` (when
   `[mcp] enabled`) with a connector token: OAuth with dynamic client
   registration, consent in the console by the person's web session, and a
   token bound to `<public_url>/mcp` (RFC 8707). Connector tokens are
   refused everywhere else, and `/mcp` refuses every other token. Each tool
   call is dispatched in process through the `/api/v1` router, so the
   normal handlers authorize and audit it; the connection's mode
   (`read_only`/`auto`) is enforced on the server, connector sessions never
   hold MFA, and destructive or MFA-guarded actions need a person. See
   D-061 to D-063.
4. **Control plane ↔ agent.** Agents dial out; servers need no inbound ports.
   mTLS with per-agent certificates from the internal CA. Every command is
   signed (Ed25519), bound to one agent id, expires, and is replay-checked.
5. **Control plane ↔ plugin runner.** Plugins run in a separate process with no
   WASI imports (no files, no network). The only imports are typed host
   functions, which the host checks with *effective permission = user ∩ plugin
   ∩ resource policy*.
6. **Control plane ↔ analytics worker.** The worker is a child process of
   `forge-server`, reached over a Unix socket (mode 0600) in a directory only
   the service user can enter (loopback TCP on Windows). Each connection
   first presents a random secret `forge-server` hands the worker on stdin at
   every start, and the protocol version. The worker trusts the control
   plane: every request is already authenticated, authorized and audited by
   `forge-server`, and log queries arrive with their scope (services and
   nodes the caller may read) already decided. The worker never reads the
   operational database; it gets the maps it needs (active nodes, service
   placements, ingest key hashes) pushed to it. See D-141.
   Its spool upload listener (`[analytics] worker_agent_listen`, default
   `0.0.0.0:55132`) is a second network-facing mTLS endpoint for agents:
   HTTP/2, client certificates verified against the Forge CA, the node
   must be in the pushed active-node list, and a file must name the
   certificate's node. Its server certificate and key are issued by
   `forge-server` at each worker start and passed on stdin, never written
   to disk. The worker does not check revocation itself; a revoked node
   leaves the active list. See D-142.
7. **Apps ↔ agent (local log listener).** The agent listens on
   `127.0.0.1:5341` and on the Docker bridge gateway (a private IPv4 the
   helper reads from the engine), never on a wildcard. A request must carry
   an ingest key whose SHA-256 matches, in constant time, a key of a
   service placed on this node (sent in heartbeat answers). Bodies are
   capped and rate-limited per key off loopback, and appended to the spool
   unparsed. The worker trusts nothing in a spool file that names a node or
   a service: the node is the certificate's, a key counts only for a
   service placed on that node, and a `forge.app` label names only a
   service placed there. See D-142.
8. **Agent ↔ helper.** The privileged helper accepts only a fixed enum of
   operations and validates every argument.
9. **Control plane → object stores.** For bucket and object operations the
   control plane calls the store's S3 endpoint directly: a managed RustFS
   member on the private network or a connected cloud service. The key is
   unsealed from the vault per request; transfers stream through the
   control plane with a concurrency cap.
10. **Control plane → dockerd (opt-in, per node).** When a node's Docker
   Engine API endpoint is enabled, dockerd also listens on TCP (default
   `2376`) with `--tlsverify`, and the control plane dials it directly. Each
   node has its own Docker CA (key sealed with the master key); dockerd's
   key is made on the node and never leaves it; client certificates live
   24 h in memory. The port is root-equivalent: only calls in the
   deny-by-default catalog are forwarded, after Cedar and a body guard on
   the control plane. See D-125.

### Enforcement points

Authorization is evaluated where an action takes effect, never only in the UI:

| Point | What is checked |
| --- | --- |
| API handlers | Cedar: principal × action × resource (with ancestry) × context (MFA, per the installation's `require_mfa`); a denial by the MFA guardrail answers `403 step_up_required` with an RFC 9470 `WWW-Authenticate` header |
| MCP endpoint | Connector token bound to `/mcp`, connection not revoked, mode (read-only/auto), destructive and MFA-guarded tools refused, rate limits; then the API handler's own checks |
| Job runner | Re-authorizes the job's principal before **every** step, so revoked roles stop running jobs |
| Agent hub | mTLS identity, revocation list, agent ↔ server binding |
| Analytics worker upload listener | mTLS identity against the Forge CA, node in the active list, file name names the certificate's node; per row, the key and `forge.app` only for services placed on that node |
| Agent log listener | Bind addresses (loopback and the Docker bridge gateway only), ingest key hash of a service placed on this node, body cap, per-key rate |
| Agent | Signature, expiry, target agent id, replay cache, local command allowlist; for `files`, path rules, protected paths and no-follow confinement (in the root helper on Linux); for `stack`, the compose policy again (bind sources after resolving symlinks) |
| Plugin host | Plugin state (enabled, not quarantined), approved permissions, Cedar check for the invoking user |

## 3. Data model

```
Platform
 └─ Workspace ─┬─ Tenant
               ├─ Cluster ─┬─ Server (node)
               │           └┄ Environment (placed here; name unique per cluster)
               ├─ Project (a product) ─ Environment
               ├─ Environment ─ Server / Service
               ├─ Server (workspace-level)
               └─ AuditLog
User ── RoleBinding(role, scope ∈ {Platform, Workspace, Tenant, Cluster, Project, Environment})
Job(resource, principal, steps…)   Plugin(id, version, state, approved perms)
```

The levels are **Project → Environment → Service**, with the same names in
the console, the API, the CLI, Cedar, the audit log and the tables. A
*project* (`projects`) is a product such as `shop`. Its *environments*
(`environments`: `test`, `staging`, `uat`…, free-text names) each belong to
one project (`environments.project_id`) and are placed in exactly one cluster
(`environments.cluster_id`). Its *services* (`services`: API, UI, Seq,
databases, containers) run inside an environment, with the runtime `systemd`
or `container`.

An environment's Cedar parents are its project and the workspace, not the
cluster: a cluster binding covers the cluster's servers, not the
environments placed in it, and a project binding covers all of the
project's environments and their services. A service's parents are its
environment and the workspace. Tenants do not contain environments.

Core tables (`src/adapters/forge-store/migrations/0001_initial.sql`, the
single baseline): `workspaces`, `tenants`, `clusters`, `projects`,
`environments`, `environment_scopes` (internal: per-cluster uniqueness scope
for environment names), `services`, `users`, `role_bindings`, `sessions`,
`oauth_codes`, `servers`, `enrollment_tokens`, `revoked_certificates`, `jobs`,
`job_steps`, `audit_log`, `plugins`, and the feature tables (files, storage,
tunnels and tunnel grants, passkeys, MCP connectors (`oauth_clients`,
`agent_connections`) and others). Every later schema change is a new
migration file after the baseline.
Columns are portable (`TEXT`, `BIGINT`), timestamps are epoch ms.

### Authorization model

* Roles are Cedar **action groups** (`Action::"role:operator"`).
* A role binding links a **policy template** (`policies/roles.cedar`) with
  `?principal = User` and `?resource = scope`. `resource in ?resource` grants
  the scope and everything below it.
* **Guardrails** (`policies/guardrails.cedar`) are `forbid` policies that
  override any permit — e.g. sensitive actions require an MFA session.
* Whether MFA is required is the operator's choice per installation
  (`[security] require_mfa`, default `true`; `install.sh --mfa`,
  `forge-server mfa on|off`). With it off, authentication marks browser
  sessions and API keys as MFA-verified, so the guardrail passes; the
  policies stay the same. Connector sessions never count as MFA-verified.
  A changed setting is audited as `config.mfa` at the next start (D-064).
* Requests that do not fit the schema fail closed.

## 4. Key flows

**Login (web).** `GET /auth/challenge` → the console solves the
proof-of-work challenge in a Web Worker (single use, 2 minutes; harder after
recent failures for that name or client address) → password (Argon2id, on
blocking threads) → TOTP when enrolled (each code works once) → session row (hash of secret) → `forge_session` HttpOnly,
Secure, SameSite=Strict cookie + `forge_csrf` readable cookie. Mutations
must echo the CSRF value in `X-CSRF-Token`. Failures are throttled per
name and address, per address and per name. Passkey and SSO sign-in skip
the challenge; the SSO callback is accepted only in the browser that
started it. A password change ends the user's other sessions, app and
connector tokens included.

**Login (apps).** `/oauth/authorize` (with an existing web session) issues a
one-time code bound to a S256 `code_challenge`; `/oauth/token` exchanges code +
verifier for a bearer access token and refresh token. Refresh tokens rotate
but expire a fixed time after the original sign-in
(`[security] refresh_token_days`, default 30).

**Login (CLI).** `forge auth login` runs the app flow above with a loopback
redirect on `127.0.0.1`; the person signs in in their browser. `forge auth
login --device` posts to `/oauth/device_authorization` (RFC 8628, first-party
client only), prints `<public address>/device` and an 8-letter code, and polls
`/oauth/token`; the person approves or denies the code in the console
(`oauth.device.approve`, audited either way), and the tokens get the MFA
state of the approving session. Requests live in memory for 10 minutes.
When a call returns `step_up_required`, the CLI on a terminal asks for a
TOTP code, posts it to `/auth/step-up`, stores the new token pair and
retries once; without a terminal it exits `5`. Step-up is audited as
`auth.step_up`, wrong codes as denials.

**Connect an AI agent (MCP).** The agent gets `401` at `/mcp` with
`WWW-Authenticate: Bearer resource_metadata=…`, reads the protected
resource (RFC 9728) and authorization server (RFC 8414) metadata, registers
at `/oauth/register` (RFC 7591), and opens `/oauth/authorize` with PKCE and
`resource=<public_url>/mcp`. The signed-in person approves on `/connect`
and picks a mode; this creates an `agent_connections` row, and the code and
tokens carry its id. Tool calls at `/mcp` become in-process `/api/v1`
requests marked with `McpDispatch`; audit events add `via` (connector,
connection, mode).

**Enrollment.** Operator creates a one-time token (`<secret>.<ca-sha256>`). The
agent fetches the CA, checks its fingerprint against the pin, generates a key
locally, sends a CSR + token. The control plane consumes the token atomically,
signs a short-lived client certificate whose identity is the server id, and
returns the command-verification public key.

**Heartbeat & commands.** The agent posts heartbeats over HTTP/2 mTLS with
jittered intervals; responses carry pending signed commands; results come back
with the next heartbeat. Disconnects back off exponentially with full jitter.

**Durable jobs.** Jobs are rows. Workers claim with a lease and a fencing token,
re-authorize, run step *n*, persist state, advance. A crashed control plane
leaves an expiring lease; on restart another worker claims the job and resumes
at the first incomplete step. Steps must be idempotent.

**Audit.** Every mutating action appends `{seq, prev_hash, …}`; `hash =
SHA-256(canonical JSON)`, signed with the control plane's Ed25519 audit key. A
`UNIQUE(seq)` constraint keeps the chain linear. `verify` recomputes the chain.

**Object storage.** Installing a managed store seals a generated root key in
the vault and submits a `storage.action` job holding only ids. The job
unseals the key into a signed `StorageAction` command; the root helper
validates the closed `StoreOp` and runs the provider script (checksum-verified
release or pinned image, per-instance service, missing-mount check) and
reports state, release and capacity. A distributed (HA) store runs one such
job per member with the identical drive layout; the store status aggregates
the members. Connected stores skip the node: Forge
checks the key with a ListBuckets (or one bucket listing) and keeps it in the
vault. Bucket and object requests are authorized at the API on the store's node
or workspace, else on the owning environment (D-107), else by a live bucket
grant for that one bucket (D-108: table `bucket_grants`, use case
`BucketGrants`), which is read on every request and never issues an S3 key.

**File explorer.** API handlers pass every request to `FileService`
(`forge-files`), which resolves the root id to a `RootSpec` (whole node,
approved directory, or container directory), checks each Cedar action the
operation needs (plus `file.node` for the whole node), refuses changes to
read-only roots, applies the protected-path policy and sends one signed
`FileAction` command through the agent hub (no job). The agent runs it in the
background; on Linux the root helper validates it again and resolves paths
component by component with `O_NOFOLLOW` and `*at` calls from the root's
directory handle (container roots start at `/proc/<pid>/root`), so no link is
followed; on Windows the agent runs it in process and refuses reparse points.
Saves and uploads land atomically after a SHA-256 check; deletes go to a trash
in the agent's protected data directory. Reads and upload chunks are at most
256 KiB per command. See D-043, D-044 and D-050.

**Compose stacks.** The API parses a compose document (YAML, server only),
interpolates it with the stack's sealed `.env`, places services on nodes
(`x-forge.node`, `x-forge.cluster`, `node.hostname ==` constraints), splits it
per node and checks each part against the stack policy; host-access settings
need `stack.privileged` (admins, MFA). A `stack.action` job per node sends
the part as a signed `StackAction`; the root helper checks it again, writes
an interpolated `compose.yaml` and runs `docker compose` with a fixed argv
and a cleared environment. Published ports bind to the node's cluster
address (or loopback) unless marked public. Deployed services register as
`compose-service` services, which Sites can expose through Nginx. See
D-058 to D-060.

**Release rollouts.** `POST /deployments` parses a `forge.deploy.yaml`
(YAML 1.2, bounded) into the typed `forge/v1` document, authorizes `deploy`
on the service (environment for a new one), `server.manage` on nodes added
or removed and `stack.privileged` for host mounts, then `DeployService`
(`deploy.rs`) writes an immutable release, plans the rollout with the pure
planner in `forge_catalog::deploy` and, under the service's
`deployment_locks` row, submits one `deploy.rollout` job. The job runs the
stored plan step by step, saving `step` after each so it resumes after a
crash:

```text
start (agent "deploy" into slot a/b on pooled ports 20000–29999)
  → gate (agent "probe" until ready, then stable)
  → [approve: deploy.approve, not the author]
  → route (slot weights → re-apply endpoints as their last applier → edges reload)
  → wait (drain / canary pause; canary gates read edge traffic: us=, rt=)
  → stop (agent "retire" on the old copy)
  → finalize (service := release, prune releases, push slot health policy)
```

A failure removes new copies that never took traffic and, when copies of
the new release served and the policy allows, starts an `auto_rollback`
deployment of the previous release. See D-126.

**Local names.** A background pass (`names.rs`, every 30 s and right after
a rename or enrollment) builds each workspace's names: nodes
(`<label>[.<cluster>].<suffix>`) and services with an active instance
(`<app>.<ns>[.<cluster>].<suffix>`). Labels are stored once and never move
(`node_names`). Addresses are mesh addresses inside a fabric, otherwise the
address the server sees each agent connect from; nothing comes from a
node's own claims. A node whose list changed, that reconnected, or that was
last synced 10 minutes ago gets a signed `HostNames` command on its next
heartbeat. The root helper (Windows: the agent) checks the suffix and every
entry again and rewrites only the marked block of `/etc/hosts` and of each
Forge container's hosts file, in place. See D-084.

**Port tunnels.** `forge tunnel open` holds a control WebSocket and opens
one data WebSocket per local TCP connection. For each, the server signs a
`TunnelOpen` (with a single-use stream token) to the node; the agent dials
only its own loopback port, then opens a **new agent-initiated data stream**:
a streaming HTTP/2 request to `/agent/v1/tunnel/<id>` on the hub, on its
own mTLS connection, outside the heartbeat. The server splices the two legs
and re-checks the owner's access every 15 s. See D-091.

**Container shells.** The console's (or CLI's) WebSocket to
`/servers/{id}/containers/{cid}/shell` authorizes `container.exec`, checks
the node (Linux, online, `container_exec` fact, reported only with a pinned helper) and
signs a
`ContainerExecOpen` (proto tag 39, single-use stream token). The agent
forwards the signed command to the root helper over its Unix socket; the
helper verifies it against its own pin (replays refused), creates a Docker
exec with a TTY as the image's user, starts it hijacked and answers one
line, then the socket carries `exec_frame` frames (data, resize, EOF, exit
code, error, ready). The agent dials `/agent/v1/exec/<session>` like a
tunnel and relays the frames unread. `container_exec.rs` in the server turns
the stream into the terminal relay's `ShellReader`/`ShellWriter`, so the
relay, the 30 s access re-check, idle and lifetime limits and revocation
through remote sessions are shared with SSH terminals. Tunnels and container
shells are the only agent-link paths that carry raw bytes rather than
commands and results. See D-114.

**Docker Engine API.** `ANY /servers/{id}/docker/engine/{path}`
(`api/docker.rs`) matches the method and path against the
`forge-docker` catalog (108 endpoints, deny by default), reads an
`inspect` body (≤ 1 MiB) and raises host-reaching settings to the admin
class, authorizes the class's action (`server.manage`, `container.manage`,
`container.exec` or `server.command` with MFA), then `DockerEngines`
(`docker_engine.rs`) opens the node's sealed CA, issues or reuses a client
certificate and forwards the call over mutual TLS to
`<host>:2376`, checking the daemon's name
`<server_id>.docker.forge.internal`. Bodies stream; attach, exec start and
BuildKit sessions are upgraded and spliced. Container inspect redacts
`Config.Env` without `container.inspect_env`; non-read calls are audited
as `docker.api`. The endpoint is set up by the `docker.endpoint.enable`
job through the root helper (`docker_tls` command: key and CSR, systemd
drop-in, restart with rollback). See D-125.

**Alerts.** Every 15 s `alerting.rs` runs one `forge_alerts::Evaluator`
pass: for each enabled rule it reads the current evidence of the rule's
subject from a `SeriesSource`, resolves each resource's workspace through
`ResourceResolver`, and feeds a `forge_rules::Tracker` per series. The
source reads observations (heartbeat measurements such as
`node.disk.max_used_percent` and health probe states) and computes
control-plane subjects from the registry on each pass: agent status and
certificate expiry per node, instance state and deployment status per
service instance, state and status per package installation, and
expiry per uploaded ingress certificate (D-118). Installations and
certificates are alarm resources but not Cedar entities: an alarm on one is
visible through its node or its workspace. Tracker events open and close
alarm episodes (one firing episode per series, enforced by a unique index);
trackers are saved after every pass. The evaluator never sends anything.
`forge_alerts::catalog` is the one definition of the known subjects and the
rule templates the API, CLI and console offer. See D-087 and D-118.

**Notifications.** The store writes an alarm transition to an outbox
(`alarm_transitions`) in the transaction that opens or closes the alarm.
Every 5 s `notifying.rs` runs `forge_alerts::notify::Notifier`: `plan`
turns each unplanned transition into one delivery per matching policy and
channel (added together with the "planned" mark, unique per alarm, event,
policy and channel) and adds due reminders; `dispatch` leases due
deliveries, sends them and records every attempt, with bounded retries.
Sending is the control plane's only notification egress: `HttpTransport`
applies the target rules to every address a name resolves to, follows no
redirect, uses no proxy and verifies TLS. Channel URLs are sealed with the
envelope; signing secrets are read from the credential vault at send time.
Nothing in this path writes to an alarm, a rule or a job. See D-110.

**Health checks.** A saved policy reaches each node of the service as
the signed app command `health`; a report under another revision makes the
server send it again. The agent runs HTTP and TCP checks itself and
applies the target rules where it connects (never link-local, metadata or
IPv6 transition addresses, nor another node of the fleet, whose addresses
come with the policy). Exec and shell checks are run by the root helper:
it keeps them from the signed command in its root-only state, the agent
names a check to run, and the helper runs it as the service's own
account, in a cgroup of its own that is killed with the run and under
Landlock (no file can be written), or with Docker exec in the
service's own container. A node that cannot confine or contain a
check does not run it. A header whose value is a vault reference is
filled in just before the probe: the agent asks
`/agent/v1/health-secrets` on its own mTLS connection, the server checks
the stored policy and what the person who saved that revision may do now,
audits, and the value stays in the agent's memory for a few minutes. It
leaves the node only over verified HTTPS, or to the workload itself.
Results travel with heartbeats and become observations. See D-085, D-105
and D-111.

**Container telemetry.** On Linux nodes the root helper watches Docker's
`/events`, follows the stdout and stderr of every running container and
samples each container's resources once a minute. This is the helper-to-
Docker boundary: the helper reads Docker as root, but the agent drives it
only through the closed, unsigned `container_telemetry` request, which reads
and whose settings (`enabled`, `exclude`) can only make collection smaller.
The agent writes the helper's batch to its analytics spool and
acknowledges it to the helper only after the lines are sealed on disk, so
only lines on disk move the helper's per-container cursor. The worker
attributes every line from the upload's client certificate, never from node
data: `forge.app = <environment>/<name>` names a service only if it is
placed on that node, then `forge.stack` or the compose project names the
stack, else the line belongs to the node (readable with `view` on it).
Lines go into `app_logs`; samples into `container_metrics_1m` and its hourly
rollup. The helper buffer is bounded (20,000 lines, 16 MiB, drop-oldest).
Agents of the previous release still send `ContainerLogBatch` and
`NodeMetrics.containers` in heartbeats, which the server forwards and
acknowledges with `container_logs_ack` once stored. See D-137 and D-142.

**Analytics spool.** Every agent appends analytics rows to one write
buffer: request bodies its local log listener accepts (unparsed, with the
key's hash), container lines and samples, and its own per-minute node row
and top processes. The buffer is sealed at 4 MiB or 10 s into a zstd NDJSON
file (`<node-id>-<file-id>.ndjson.zst`, the id a UUIDv7): `.tmp` → fsync →
rename `.ready` → directory fsync. An uploader posts ready files oldest
first, one at a time, to `POST /agent/v1/spool` on the worker (HTTP/2 mTLS,
header `x-forge-spool-file`), and deletes a file only after the
`SpoolAck` names it. The worker admits at most `worker_max_uploads`
imports and answers `429` at once beyond that; the agent backs off with
jitter. The worker decompresses as a stream (64 MiB cap), parses rows with
the same functions as the off-fleet path, applies the trust rules, and
hands the rows to the store's `SpoolFileWriter`. ClickHouse writes one
insert per table with `async_insert=1, wait_for_async_insert=1` (at least
once); DuckDB commits each family's rows with the file id in one
transaction and skips families that already hold it (once per table,
D-143). The worker then records the file id in `<data_dir>/analytics-worker/spool-ids/seen.log`
(last 64 per node) before acknowledging. Bounds on the node: 512 MiB cap
(oldest file dropped and counted), 256 MiB free-disk floor (listener
answers `429`), 72 h maximum age. A crash loses the write buffer, at most
the seal interval of app logs. Heartbeats carry the live sample and
`SpoolStatus` only; their answers bring `AnalyticsAgentConfig` (worker URL,
spool and listener tuning, key hashes of services placed on the node).
Apps outside the fleet still post to `POST /api/events/raw` on
`forge-server`, which checks the key and forwards the body to the worker.
See D-142.

**Plugins.** Upload manifest + component + publisher signature → verify →
admin approves requested permissions → install → enable. Invocation spawns the
runner with budgets; budget exhaustion stops it and quarantines the plugin.

## 5. Extension points for later phases

| Phase | Extension point already in place |
| --- | --- |
| 2 Access | `terminal.open` action, MFA guardrail, agent tunnel over the existing link, gateway as separate process |
| 3 Deployments | `Service`/`Release` types, `deploy` action, job engine with resumable steps, `ServiceManager` trait in the agent |
| 4 Proxy | Job steps (validate→stage→apply→verify→revert) and signed agent commands |
| 5 DNS + DB | `db.admin`/`secret.read` actions, envelope encryption, plugin host |
| 6 Reliability | `KeyProvider` trait (TPM/KMS/OpenBao), audit export |
| 7–8 Containers/K8s | New job kinds + plugins; orchestrators keep scheduling |
| 9 Ecosystem | Plugin catalog sources, staged rollout via installed/desired/active |

## 6. Lightweight profile

One process + SQLite, no Redis, no broker. The plugin runner only exists while
a plugin runs. The agent uses a single-threaded Tokio runtime, one HTTP/2
connection, and bounded buffers. Measured numbers are in
[`BENCHMARKS.md`](BENCHMARKS.md).
