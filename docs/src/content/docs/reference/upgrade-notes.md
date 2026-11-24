---
title: Upgrade notes
description: What migrations and security changes do to existing data and settings when you update Forge, and what to review afterwards.
sidebar:
  order: 7
---

Most updates only add tables and columns. This page lists the changes that
also change existing data or break existing scripts, and the security changes
that can refuse settings you saved before, so you know what to check after
you update. Forge runs migrations automatically on start, on SQLite and
PostgreSQL alike.

## Any address: no fixed console URL, agents pin the server key

- **`public_url` is optional.** An existing `forge.toml` keeps it, and it
  stays the pinned address for emails, alerts, SSO and AI connectors. The
  console now also answers on every other name and IP of the server, and
  links built in the console (Add node commands, approval and device links)
  use the address you are on. A `public_url` of `0.0.0.0`, without a scheme,
  or with a path stops the server at start: fix or remove it.
- **Allowed hosts default to `*`.** Nothing is refused after the update.
  Narrow the list in **Settings → Server** if Forge has a domain; see
  [Allowed hosts](/docs/reference/configuration/#allowed-hosts).
- **`trusted_proxies` of `0.0.0.0/0` or `::/0` stops the server at start.**
  List your proxies instead. Trusted proxies now also set the host and scheme
  through `X-Forwarded-Host` and `X-Forwarded-Proto`.
- **Same-origin checks.** WebSocket and `/mcp` requests need a browser
  `Origin` equal to the address the request came in on, or the pinned
  address. A proxy that rewrites `Host` without sending `X-Forwarded-Host`
  from a trusted address breaks live updates on names other than the pinned
  one; see [Reverse proxy headers](/docs/reference/configuration/#reverse-proxy-headers).
- **HSTS** is sent only on HTTPS responses, no longer whenever
  `cookie_secure = true`.
- **Control-plane key.** On first start the server creates a control-plane
  key pair (audited as `agent_key.create`) and serves the agent and
  analytics listeners with a certificate for it. Renewal keeps the key.
- **Existing agents** keep checking the CA and the host name until they
  update. An updated agent receives the key in its next heartbeat reply over
  its existing link and pins it; no re-enrollment is needed. Agents that are
  not updated keep working: their enrolled URL still matches the
  certificate's names.
- **Analytics uploads.** New agents dial the worker on the host of their own
  `server_url`. Older agents keep using `analytics_url`, which the server
  still sends.
- **Passkeys** are bound to the name they were registered on, as before when
  that name was `public_url`. On another name, sign in with a password and
  code.

## Agents update to the version in the agent folder's manifest

- **`agent_dist_dir` has a `MANIFEST.json`** with the version of the agent
  binaries there and their SHA-256. Agent updates and rollouts go to that
  version, not to the server's own version. Release packages and the
  self-updater write it; a server update replaces it with the binaries and
  a rollback restores it.
- **A file that does not match the manifest is not handed out.** If you
  copy your own agent builds into the folder, write the manifest again
  (`python3 scripts/dev/dist_manifest.py <dir> <version>`) and restart
  `forge-server`.
- A folder without a manifest works as before: its builds count as the
  server's version.

## Release candidates and dev builds sort below their release

- **Forge orders its own versions like semantic versioning.** A release
  candidate or dev build of `X.Y.Z` (`1.0.0-rc-01`, `1.0.0-dev.<stamp>`) is
  older than `X.Y.Z`. Before, `1.0.0-rc-01` sorted above `1.0.0`, so a
  server on a release candidate was never offered the final release.
  Package versions on nodes keep Debian ordering.
- **An agent is outdated only when it runs an older version** than the
  control plane hands out. An agent on a newer version is listed as
  **ahead** and no rollout, update or automatic update touches it. Before,
  any other version counted as outdated, so a rollout could downgrade an
  agent.
- **Local dev builds are stamped with the next patch version**
  (`0.1.1-dev.<stamp>` while `Cargo.toml` says `0.1.0`), by
  `scripts/dev/local_lab.py`, `scripts/dev/dev.py` and
  `scripts/setup_testing/publish.py`. A dev agent stamped `0.1.0-dev.<stamp>`
  is older than a `0.1.0` server, so a rollout updates it.

## Analytics uploads: current certificates, grouped ClickHouse inserts

- **Update `forge-server` and `forge-analytics-worker` together.** The
  control protocol between them is now version 2. The release package and
  the self-updater ship both; a worker binary left from the older release
  is marked `incompatible` and not started.
- **Only a node's current certificate uploads.** The worker refuses an
  upload whose client certificate is not the node's current one (`403`).
  Agents switch to a renewed certificate on their own; a node that still
  uses a replaced certificate keeps its data in the spool until it
  reconnects.
- **The worker's listener certificate is renewed in place**, at two thirds
  of its 90 days, without a worker restart.
- **ClickHouse and the embedded engine:** a migration sets
  `non_replicated_deduplication_window = 10000` on the six analytics
  tables, so spool files are stored once per table. An insert of rows
  identical to one of the last 10,000 inserts of a table is now dropped as
  a duplicate. Forge never writes such inserts on purpose: log rows carry
  unique ids, and the minute tables already replace duplicate rows.
- **Compose stack services get `FORGE_LOG_URL`** and a
  `forge-node:host-gateway` host entry at their next deploy, unless they
  set them. See
  [Send and search service logs](/docs/guides/application-logs/).

## DuckDB analytics engine

`[analytics] engine = "duckdb"` is new, and it is now the default. It runs
DuckDB inside the analytics worker, so there is no engine process and
nothing to download.

- Existing installs keep their engine: the installer writes `engine` into
  `forge.toml`, and Forge does not change it.
- A `forge.toml` with **no** `engine` key under `[analytics]` used to mean
  `off`; it now means `duckdb`. Set `engine = "off"` to keep analytics off.
- New installs get `duckdb` from `install.sh` unless you pass `--analytics`.
  The Windows installer still defaults to `off`.
- `worker_max_uploads` now defaults to `32` with `duckdb` (`8` otherwise).

- New key: `[analytics] duckdb_memory_mb` (default `256`). With `duckdb`,
  `worker_memory_limit_mb` must be at least `duckdb_memory_mb` plus 256, or
  the server does not start.
- Switching an install to `duckdb` starts with empty history: Forge does not
  copy rows between engines. The old engine's data stays on disk.
- The binary of `forge-analytics-worker` grows by about 50 MB (DuckDB is
  linked in). `forge-agent` does not change.
- `GET /api/v1/analytics/settings` gains a `jobs` list (empty for other
  engines).

## Nodes upload analytics straight to the worker

Agents now keep a disk spool and upload node history, top processes,
container output and application logs to the analytics worker on
`55132/tcp`, not in heartbeats.

- **Open `55132/tcp`** from the nodes to the control plane. Re-running the
  installer opens it in firewalld, ufw or Windows Firewall; open it in any
  network firewall yourself. Until it is open, nodes keep working, but their data
  waits in the spool (512 MiB, 72 hours by default) and the oldest files
  are dropped after that. See [Network ports](/docs/reference/network-ports/).
- New keys: `[analytics] worker_agent_listen`, `worker_agent_url`,
  `worker_max_uploads`, and the sections `[analytics.spool]` and
  `[analytics.listener]`. See [Configuration](/docs/reference/configuration/#analytics).
- If nodes reach the server under a name that is not the host of
  `agent_public_url`, set `worker_agent_url`, and keep its host in
  `agent_hostnames`.
- Services get `FORGE_LOG_URL` at their next deploy. Point their loggers
  at it to log through the node; see
  [Send and search service logs](/docs/guides/application-logs/). The
  server URL keeps working for apps outside the fleet.
- Agents of the previous release keep sending telemetry in heartbeats, and
  the server forwards it, until you [update them](/docs/guides/update-agents/).

## Analytics runs in its own process

Analytics now runs in `forge-analytics-worker`, a child process that
`forge-server` starts and supervises. There is no new service unit.

- The release package and the self-updater install the new binary next to
  `forge-server`. If you update with the self-updater of an older release,
  it does not know the new file: the Analytics settings page then shows the
  worker as not running, and analytics endpoints answer
  `503 analytics_worker_down`. Run the update once more from the new
  release, or install the package with `install.sh`, to add the worker.
- New keys under `[analytics]`: `worker_memory_limit_mb` (default `2048`)
  and `worker_binary`. See
  [Configuration](/docs/reference/configuration/#analytics).
- `GET /api/v1/analytics/status` and `GET /api/v1/analytics/settings` gain a
  `worker` object. While the worker restarts, analytics endpoints answer
  `503 analytics_worker_down`; retry them like `analytics_unavailable`.
- `ingest.max_queued_mb` now counts parsed events in the worker's memory,
  not request bodies. The same value admits less raw data than before.

## Analytics engine limits without cgroups

Forge no longer creates a cgroup for the embedded analytics engine, and
`forge-server` is never limited.

- The CPU cap (`cpu_quota_vcpu`) now pins the engine to whole CPUs, rounded
  up, and sizes its thread pools to match. Restart the engine once after the
  update so the pools follow the cap.
- The hard memory limit is removed. The memory budget
  (`embedded_memory_mb`, setting `memory_budget_mb`) is the engine's own
  cap. A stored `memory_limit_mb` override is dropped on start, and
  `memory_limit_mb` in `forge.toml` is ignored with a warning; delete it.
- `Delegate=cpu memory` is no longer needed. You can remove it from
  `forge-server.service` or its drop-in, then run
  `sudo systemctl daemon-reload && sudo systemctl restart forge-server`.
- With an external ClickHouse, `PUT /api/v1/analytics/settings` now refuses
  `cpu_quota_vcpu` and `memory_budget_mb` with `409 managed_externally`
  instead of reporting them as `external`. The apply status `external`, the
  confirmation `memory_limit_below_usage` and the usage fields
  `hard_memory_limit_bytes` and `cpu_quota_vcpu` are gone; `usage.allowed_cpus`
  is new.

## Projects, environments and services: fresh database required

Applies to every installation. This is a breaking release: the database
schema starts again from one baseline migration (`0001_initial.sql`), and
the analytics schema starts again at version 1. Forge cannot upgrade a
database or an analytics store made by an earlier build.

:::caution
Install this release on a new, empty data directory. Back up the old data
directory first, then create projects, environments and services again (the
[seed scripts](/docs/development/docker-nodes/) and
`forge services create --file` help). Pointing this release at an old
database is not supported.
:::

The vocabulary is now **Project → Environment → Service** everywhere: the
console, the REST API, the CLI, Cedar and the audit log. There are no
aliases for the old names. See
[Projects, environments and services](/docs/concepts/projects-environments-services/).

| Area | Before | Now |
| --- | --- | --- |
| REST API | `/api/v1/app-groups` | [`/api/v1/projects`](/docs/reference/rest-api/#projects) |
| REST API | `/api/v1/namespaces`, `PUT /namespaces/{id}/app-group` | [`/api/v1/environments`](/docs/reference/rest-api/#environments), `PUT /environments/{id}/project` |
| REST API | `/api/v1/applications/…` (instances, actions, deploy, health, health-summary, storage, releases, deployments, rollback, slots, log-keys) | [`/api/v1/services/…`](/docs/reference/rest-api/#services) |
| Fields and query parameters | `app_group_id`, `namespace_id`, `app_id`, `application_id`, logs `application=` | `project_id`, `environment_id`, `service_id`, logs `service=` |
| Service runtime | `service`, `job`, `container`; `schedule` field; `run` action | `systemd` or `container`. The job runtime, `schedule` and `run` are gone (the durable jobs engine is unchanged) |
| CLI | `forge applications`, `forge namespaces`, `forge apps` | [`forge projects`](/docs/reference/cli/projects/), [`forge environments`](/docs/reference/cli/tenants-environments/), [`forge services`](/docs/reference/cli/services/) |
| CLI flags | `--namespace`, `--app-group`; `--app` on `forge alerts create-rule` and `forge alerts silences create`; `--application` and `--kind application` on `forge nodes files roots add` | `--environment`, `--project`, `--service`, `--service` and `--kind service`. `forge tunnel --app` is unchanged |
| Cedar entity types | `AppGroup`, `Namespace`, `Application` | `Project`, `Environment`, `Service` ([Cedar schema](/docs/reference/cedar-schema/)) |
| Role and access scopes | `app_group:`, `namespace:`, `application:` | `project:`, `environment:`, `service:` |
| Audit actions | `app_group.*`, `namespace.{create,rename,delete,move,set_app_group}`, `application.*` | `project.*`, `environment.{create,rename,delete,move,set_project}`, `service.*` ([Audit](/docs/concepts/audit/)) |
| Alert and silence scopes | `applications`, `namespace` | `services`, `environment` |
| Tag kinds, file root kind | `namespace`, `application` | `environment`, `service` |
| MCP tools | `forge_list_namespaces`, `forge_list_applications`, `forge_get_application` | `forge_list_environments`, `forge_list_services`, `forge_get_service` |
| Console | **Applications**, **Environments**, `/applications`, `/environments`, **Software** | **Projects** (the project explorer), `/projects`, `/services/<service-id>`, **Packages** |

Environment names are now free text (1–100 characters), unique within a
cluster. Unit and container names on the nodes keep the form
`forge-app-<environment>-<service>`.

## Server data is owner-only

Applies to Linux servers. The installer and the server make the data
directory and its database, keys, CA, backups and staged updates readable
by the `forge` service account only (`0700` directories, no group or other
permissions on files). Earlier installs could leave `forge.db` at `0644`
inside a `0700` directory. Nothing to do: re-running the installer or the
next start fixes it. The installer re-owns everything under the data
directory to `forge:forge` (except `initial-admin-password`, which stays
root's) and tightens the modes, so files placed there by hand change owner
too. If the server log says `could not make this file
owner-only`, run the command it names. Tools that read the database or
backups as another user (for example a backup agent) need to run as
`forge` or as root.

## Security review of authorization: what may need attention

- **Automatic package patches** now run as the person who set the policy to
  `auto_patch`, with that person's current roles. Installations set to
  `auto_patch` before this update have no recorded owner and stop patching
  automatically: set the policy again with `forge packages policy
  <installation-id> auto_patch` or in the console. The server log says
  `auto_patch has no recorded owner` for each one.
- **API keys** made before this update keep their expiry. Keys without one
  stay valid; review keys that show **never** under **Expires**. New keys
  expire after 90 days unless you choose **never**.
- **Credential vault**: adding, changing and deleting items needs an
  MFA-verified session (`secret.manage`).
- **Sign-in names**: changing your username or email needs your current
  password, or a step-up if you use MFA.
- **Live events** (`/api/v1/events`) close within 30 seconds when the
  sign-in behind them ends; the console reconnects.

## Container services: mounts outside the service folder need an administrator

Applies to container services with a volume whose host path is outside
`/srv/forge/apps/<environment>/<app>/` and outside the node's
[approved file roots](/docs/guides/approve-file-roots/). Running containers keep
running. The next deploy of such an app needs `stack.privileged` (**Admin**
or **Platform admin**) with an MFA-verified session; for a **Developer**,
an **Operator**, an API key or a connector it returns `403`. To keep deploys
open to them, move the data into the app's folder or approve its folder as a
file root on the node. See [Services](/docs/guides/services/#configuration-sections).

## WireGuard remote access removed

Applies to every install that used **Network ▸ Remote access**. Access
gateways, device profiles and the `forge remote-access` commands are gone:
the API port is the only way in. Port tunnels take over, now pointed at a
service (Forge service, stack service, database or SSH) instead of a bare
port, opened by a role on the environment or by an explicit grant under
**Administration ▸ Tunnels ▸ Grants**. The migration drops the
`remote_access` and `access_profiles` tables, removes the Cedar actions
`network.connect`, `network.access` and `network.manage`, and replaces the
`tunnel.ports` allow-lists with the service catalogue. After the agents
update, the `forge-access-watchdog` timer is removed from gateway nodes.
Devices still holding a WireGuard configuration cannot connect: hand out
`forge tunnel open` lines instead. See
[Open a port tunnel](/docs/guides/port-tunnels/).

## Cluster network: routed mode removed

Applies to clusters whose network was saved with **Routed (no encryption)**.
Forge never applied routed mode to nodes, so nothing changes on them. The
network now shows as **encrypted mesh**, and the next **Apply network** sets
up the WireGuard mesh on its members and saves the mode as `mesh`. Saving a
network with `"mode": "routed"` through the API returns `400`. See
[Cluster network](/docs/guides/cluster-network/).

## Terminal certificates bound to one node

Applies to Linux nodes with the browser terminal enabled. After its agent
update, each node accepts only certificates whose principal is
`forge.<server-id>.<login>`; Forge issues those as soon as the node reports
the new setup.

:::caution
If a node also trusts your own SSH user CA, its certificates stop working on
that node after the update, because the principals command applies to every
CA. See [Terminal](/docs/guides/terminal/#what-the-installer-changes-on-a-linux-node).
:::

## Root helper verifies Forge's signature

Applies to Linux nodes. Nothing changes on the server or in the database.

| What | After the update |
| --- | --- |
| Nodes enrolled with this release or later | The installer pins Forge's command key as root. The helper runs only commands Forge signed for this node |
| Nodes enrolled earlier | After their agent update, the new helper pins the key from the agent's `identity.json` the first time it starts, then enforces signatures |
| Scripts that talk to `/run/forge-agent-helper.sock` directly | Refused once the helper has a pin. Use Forge's API instead |

:::caution
First-use pinning trusts a file the unprivileged agent user owns. If a node
might have been compromised before this update, revoke it on the node page
and add it again with a new token, so the installer pins a fresh enrollment
as root.
:::

## Agent command allowlist: removals stay removed

Applies to nodes where you removed names from `allowed_commands` in
`agent.toml`. From this update on, re-running the node installer adds only
command names that are new in that release, and remembers what it offered in
`offered_commands`.

:::caution
The first update after this change cannot tell your earlier removals apart
from names that were never offered, so it adds every missing name once.
After updating, open `/var/lib/forge-agent/agent.toml` on each restricted
node, remove the names again and restart the agent. Later updates keep them
removed.
:::

## Sign-in hardening

Applies to every installation. Nothing in the database changes, but some
sign-ins behave differently after the update.

| What | After the update |
| --- | --- |
| Changing your password | Ends every other sign-in of your account: browser sessions, app tokens and AI connector tokens. The browser that made the change stays signed in. API keys keep working |
| Authenticator codes | Each code works once. Wait for the next code to sign in again or to verify a second time |
| App refresh tokens | Expire 30 days after the original sign-in (`[security] refresh_token_days`), however often they are refreshed. Apps then ask you to sign in again |
| Failed sign-ins | Counted per account and address (10), per address (50) and per account (100) in 15 minutes. Behind a reverse proxy, list it in `[security] trusted_proxies` so Forge sees the client address |
| Single sign-on | The callback must reach the browser that started the sign-in. A sign-in started in one browser and finished in another fails; start it again |

:::caution
If Forge runs behind a reverse proxy on another host, set `trusted_proxies`
before updating. Otherwise all failed sign-ins count against the proxy's
address, and 50 failures in 15 minutes block sign-in through that proxy for
everyone for a while. A proxy on the same host (loopback) gets no
address-wide limit, so sign-in keeps the per-account limits until you list it.
:::

## Custom Nginx blocks: supported directives only

Applies to sites and cluster proxy profiles that have a custom block. Nginx
reads these blocks as root on the edge, so Forge now accepts only directives
from the [supported list](/docs/guides/sites-and-dns/#custom-blocks). Directives
that take a file path, change the Nginx process or load code are refused,
for example `root`, `alias`, `try_files`, `auth_basic`, `access_log`,
`include` and every `*_by_lua*` form. Unix sockets (`unix:`) are refused too.

| What | After the update |
| --- | --- |
| Saved block with a directive outside the list | Stays saved, but **Apply** (and saving it again) fails with `400` and names the directive |
| Saving or applying a non-empty block | Needs `server.command` (MFA) on every edge of the site or cluster |
| Let's Encrypt renewal of a site with a custom block | Runs as the person who last applied the site, who now also needs `server.command` on the edges |

:::caution
Review custom blocks before the next apply. Open each site and each
cluster's proxy profile, remove refused directives, and apply again as a
user with `server.command`. Serve static files from a service instead
of `root` or `alias`.
:::

## Compose stacks and containers: stricter host checks

Applies to compose stacks, containers made with **Create container**, and
container services with volumes. Saved definitions stay, but the next check,
**Deploy** or redeploy refuses them until you change them.

| What | After the update |
| --- | --- |
| Read-write bind that contains the stack folder, such as `.:/app` | Refused, because it exposes `compose.yaml` and `.env`. Mount it read-only (`.:/app:ro`) or mount a subfolder |
| Bind, `env_file`, `label_file` or config/secret `file` inside another read-write bind of the same stack | Refused |
| A bind path with a symbolic link in it | Refused on the node, except root-owned system links such as `/var/run` |
| Network named `host` (also as an external network), `container:` or another stack's `service:` mode, `pid`/`ipc`/`uts`/`userns`/`cgroup` set to `container:`, hooks with `privileged: true`, `use_api_socket` | Needs `stack.privileged` |
| Unknown service keys and `provider:` services | Refused. `x-*` extension keys still work |
| **Create container** with network `host` | Needs `stack.privileged`; `none` and `host` become `network_mode` |
| Container app volume on a Docker socket, a system path or Forge's own folders | Refused |

See [Compose stacks](/docs/concepts/compose-stacks/) and the
[compose extensions reference](/docs/reference/compose-extensions/).
