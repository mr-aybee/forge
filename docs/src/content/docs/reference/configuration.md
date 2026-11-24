---
title: Configuration
description: Every forge.toml key and environment override.
sidebar:
  order: 1
---

`forge-server --config forge.toml` (or `FORGE_CONFIG`). Unknown keys are
rejected, so typos fail loudly.

| Key | Default | Description |
| --- | --- | --- |
| `data_dir` | `.forge-data` | Database, keys, CA, plugin artifacts |
| `database_url` | `sqlite:<data_dir>/forge.db` | Older one-line form: `sqlite:…` or `postgres://…`. When set it wins over `[database]`. Env: `FORGE_DATABASE_URL` |
| `api_listen` | `127.0.0.1:8080` | API + UI listener (plain HTTP; front with TLS) |
| `agent_listen` | `0.0.0.0:7443` | Agent mTLS listener |
| `agent_hostnames` | `["localhost", "127.0.0.1"]` | Names/IPs in the agent listener certificate |
| `public_url` | `http://localhost:8080` | External API/UI URL: the direct address of the console. Default for the [public address](#public-address) in links, and always an allowed browser origin. Also the WebAuthn relying party, the OAuth issuer for AI connectors and the SFTP host default |
| `agent_public_url` | `https://localhost:7443` | Printed in enrollment instructions |
| `node_public_url` | `public_url` | API URL as reachable from nodes (installer downloads) |
| `agent_dist_dir` | `<data_dir>/dist` | Agent binaries handed to nodes: `forge-agent-linux-<arch>` and `forge-agent-helper-linux-<arch>` (`x86_64`, `aarch64`), served at `/agent/download/linux/<arch>/<binary>`, and `forge-agent-windows-x86_64.exe`, served at `/agent/download/windows/x86_64/forge-agent`. Releases ship only the Linux builds; a Windows agent build comes only from a source build of your own, which is not verified. The server hashes the builds at start-up and bakes the hashes into the installers, so restart it after you add or replace a build. `MANIFEST.json` in the folder (`{"version": "<version>", "files": {"<file>": "<sha256>"}}`) names the version of these builds: agent updates go to that version, not the server's. A build missing from the manifest, or with another hash, is not served; an unreadable manifest serves nothing. Without a manifest, the builds count as the server's own version. Release packages and the self-updater write it; for your own builds run `python3 scripts/dev/dist_manifest.py <dir> <version>` |
| `cookie_secure` | `true` | `Secure` flag on session cookies. With `false`, cookies still get `Secure` on requests that a [trusted proxy](#security) reports as HTTPS (`X-Forwarded-Proto: https`) |
| `environment` | `"production"` | Where this control plane runs: `production`, `staging`, `development` or `test`. It does not decide whether MFA is required (see `[security]`). Env: `FORGE_ENVIRONMENT`. Installer: `--environment <name>` |
| `dev_skip_mfa` | `false` | **Deprecated.** `true` is the same as `[security] require_mfa = false`, in any environment, and logs a deprecation warning at start. `forge-server mfa on\|off` sets it to `false` when it edits the file. Env: `FORGE_DEV_SKIP_MFA=1` (or `true`). Installer: `--dev-skip-mfa` (alias of `--mfa off`) |
| `storage_allow_loopback` | `false` | Lets object-store endpoints use loopback addresses (`127.0.0.1`, `::1`, `localhost`): for labs, development and tests. Link-local and cloud-metadata addresses (`169.254.0.0/16`, `fe80::/10`, `fd00:ec2::254`) are refused either way, also after DNS resolution. Env: `FORGE_STORAGE_ALLOW_LOOPBACK=1` |
| `ui_dir` | none | Serve the built UI from this directory |
| `heartbeat_hold_secs` | `25` | Long-poll hold for idle agents |
| `offline_after_secs` | `90` | Agents unseen this long are marked offline |
| `plugin_runner` | next to `forge-server` | Path of `forge-plugin-runner` |
| `trusted_publishers` | `[]` | `[{ publisher, public_key }]` |
| `oidc` | none | See [SSO](/docs/guides/sso-oidc/) |
| `jobs.lease_secs` | `30` | Job lease length |
| `jobs.concurrency` | `8` | Jobs run in parallel per process |
| `jobs.poll_ms` | `500` | Idle poll interval |

### Security

The `[security]` section sets whether MFA is required and whether password
sign-in needs a login challenge. Both are on by default. See
[MFA is your choice](/docs/security/#mfa-is-your-choice).

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `security.require_mfa` | boolean | `false` | MFA is each user's choice either way: a user with an enrolled factor is always asked for it at sign-in. `true`: actions under the `guard:sensitive-requires-mfa` guardrail need an MFA-verified session, and revoking a node asks for an authenticator code. `false` (the default) counts every browser session and API key as MFA-verified, in any environment: no action asks for MFA or step-up. MCP connector sessions never count as MFA-verified. Change it with `forge-server mfa on\|off` and restart. Env: `FORGE_REQUIRE_MFA` (`false`, `0`, `off` or `no` turn it off; any other value turns it on). Installer: `--mfa required\|off` |
| `security.login_challenge` | boolean | `true` | Password sign-in (`POST /api/v1/auth/login`) needs a solved proof-of-work challenge from `GET /api/v1/auth/challenge`. Passkey and SSO sign-in are not affected. Env: `FORGE_LOGIN_CHALLENGE` (`false`, `0`, `off` or `no` turn it off; any other value turns it on) |
| `security.captcha_after_failures` | integer (failed sign-ins) | `2` | After this many failed password sign-ins for a sign-in name, or from one client address, within 15 minutes, the next sign-in also needs the characters from an image CAPTCHA (`GET /api/v1/auth/captcha`). Failures count per name and per address, loopback included; a successful sign-in clears the name's count, not the address's. `0` turns it off. Passkey and SSO sign-in are not affected. Env: `FORGE_CAPTCHA_AFTER_FAILURES` |
| `security.refresh_token_days` | integer (days) | `30` | Absolute lifetime of an app's refresh token, counted from the original sign-in. Refreshing gives new tokens but never extends it; after it the app must sign in again. `1` to `365` |
| `security.allow_elevated_tokens` | boolean | `true` | API keys and service-account tokens marked elevated count as MFA-verified, so CI can run MFA-guarded actions without a prompt. `false` refuses new elevated tokens (`elevated_tokens_disabled`) and makes existing ones act as normal tokens. Change it with `forge-server elevated-tokens on\|off` and restart. Env: `FORGE_ALLOW_ELEVATED_TOKENS` (`false`, `0`, `off` or `no` turn it off) |
| `security.trusted_proxies` | list of IP addresses or CIDRs | `[]` | Reverse proxies, such as ingress edge nodes, whose `X-Forwarded-For` and `X-Forwarded-Proto` headers Forge trusts. `X-Forwarded-For` gives the client address for per-address sign-in limits; `X-Forwarded-Proto: https` makes session cookies `Secure` when `cookie_secure = false`. From any other peer both headers are ignored. With the default, Forge uses the TCP peer and ignores the headers. Installer: `--trusted-proxies <list>`. Example: `["127.0.0.1", "10.0.0.0/8"]` |

```toml
[security]
require_mfa = false
login_challenge = true
captcha_after_failures = 2
refresh_token_days = 30
allow_elevated_tokens = true
trusted_proxies = ["127.0.0.1"]
```

At every start the server logs an `MFA policy` info line with
`required_for_guarded_actions`. When the setting differs from the last start, the audit
log records `config.mfa` with `old` and `new` (`required` or `off`), as actor
`system:forge`.

:::caution
With `require_mfa = false`, one stolen password gives root on every node that
user can reach, unless the user enrolled MFA for sign-in. Set it to `true` on
any Forge that clients or production systems depend on.
:::

### Access lists and rate limits

`[security.access]` limits which client addresses reach Forge, and
`[security.rate_limit]` limits how often. Agents connect on their own port and
are not affected. The client address is the TCP peer, or `X-Forwarded-For`
from a `trusted_proxies` peer.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `security.access.deny` | list of IP addresses or CIDRs | `[]` | Refused everywhere (`403`), before any other check |
| `security.access.allow` | list of IP addresses or CIDRs | `[]` | When not empty, only these reach the console, the API, MCP and OAuth. Must include `127.0.0.1`, `::1` and every `trusted_proxies` entry, or the server refuses to start |
| `security.access.webhooks_allow` | list of IP addresses or CIDRs | `[]` | When not empty, only these may call deploy webhooks (`/hooks/*`). `allow` does not apply to webhooks, so CI can deploy while the console is closed |
| `security.access.nodes_allow` | list of IP addresses or CIDRs | `[]` | When not empty, only these may fetch the node installers and agent downloads (`/install.sh`, `/install.ps1`, `/agent/download/*`). `allow` does not apply to them |
| `security.rate_limit.enabled` | boolean | `true` | Limits requests per client address |
| `security.rate_limit.per_ip_per_minute` | integer | `1200` | Requests per minute from one address; an IPv6 `/64` counts as one address. At least `60`. Beyond it: `429` with `Retry-After: 60`. `/api/health` is never limited |

```toml
[security.access]
allow = ["127.0.0.1", "::1", "10.0.0.0/8"]
deny = ["198.51.100.0/24"]
webhooks_allow = ["192.0.2.0/24"]   # your CI runners
nodes_allow = []

[security.rate_limit]
enabled = true
per_ip_per_minute = 1200
```

Agent enrollment is limited separately to 20 attempts per minute per address,
and at most 16 agent downloads stream at once.

:::caution
An `allow` list that misses the address you manage Forge from locks you out
of the console. Keep a way in (SSH to the control plane) when you change it.
:::

### Public address

The public address is the URL people use to reach the console. Forge puts it
in every link it sends: alert emails, webhook and Slack payloads
(`links.alarm`), approval links for AI connectors, device sign-in
(`verification_uri`), the OIDC redirect and the OpenAPI `servers` entry. It is
a runtime setting, not a `forge.toml` key.

| Source | Precedence | How to change it |
| --- | --- | --- |
| Runtime setting `server.public_address` | 1 (wins when set) | Console **Administration → Settings**, `forge settings public-address set <url>`, `forge-server public-address set <url>`, installer `--public-address <url>` |
| `public_url` in `forge.toml` | 2 (fallback) | Edit the file and restart |

| Rule | Value |
| --- | --- |
| Type | string, an origin: `http` or `https`, a host and an optional port |
| Default | unset (links use `public_url`) |
| Refused | anything else: a path (even `/forge`), a query, a fragment, a user name or password, another scheme, more than 255 characters |
| Stored as | lowercase host, default port dropped: `https://Forge.Example.com:443/` becomes `https://forge.example.com` |
| Applies | at once, for changes made through the API or console. `forge-server public-address` writes the database only: restart a running server to apply it there. The OIDC redirect URL is read at start, so restart after a change when you use SSO |
| Allowed browser origins | `public_url` and the public address. WebSocket endpoints (events, terminal, tunnels) and `/mcp` refuse any other `Origin`. The `Host` header is never used |
| Permission | read: `view` on the platform; change: `system.update` on the platform and an MFA-verified session. Audited as `server.public_address.update` and `server.public_address.clear` |

```sh
forge settings public-address set https://forge.example.com
```

:::note
Passkeys are bound to the host of `public_url` (the WebAuthn relying party).
A passkey works only on that host: on the domain, sign in with a password
and code, or set `public_url` to the domain.
:::

See [Publish Forge on a domain](/docs/guides/publish-forge-on-a-domain/).

### Updates

The `[update]` section controls how the server looks for new Forge releases
and offers them under **Update to …** in the console. A release is installed
only when its `SHA256SUMS.sig` verifies against `public_key`.

| Key | Default | Description |
| --- | --- | --- |
| `update.enabled` | `true` | Check the feed every six hours and offer updates in the console |
| `update.feed_url` | `https://api.github.com/repos/mr-aybee/forge/releases/latest` | GitHub "latest release" API URL, or a mirror that answers in the same shape |
| `update.releases_url` | `feed_url` without its trailing `/latest` | GitHub-compatible "list releases" URL. The server reads `<releases_url>?per_page=50` for the version list and `<releases_url>/tags/v<version>` for one release |
| `update.public_key` | Forge's release key (built in) | Base64url Ed25519 public key that release checksums are signed with. Set it only for a mirror that signs its own builds |
| `update.agent_batch_timeout_secs` | `1800` | Seconds one batch of an agent rollout waits for its offline nodes to come online (`1` to `86400`). The rollout itself waits, so an offline node holds no job worker; a node still offline at the end is skipped. The batch itself may run 10 minutes longer, for a node that came online at the last moment to restart on the new version |

```toml
[update]
enabled = true
feed_url = "https://releases.example.com/repos/<owner>/<repo>/releases/latest"
public_key = "<base64url-ed25519-public-key>"
```

:::caution
Changing `public_key` changes which signatures the server trusts for its own
updates. Use a key only from a source you control.
:::

### SFTP gateway

The `[sftp]` section runs Forge's own SFTP server for
[temporary access](/docs/guides/sftp-access/). It is off by default.

| Key | Default | Description |
| --- | --- | --- |
| `sftp.enabled` | `false` | Run the gateway and allow temporary access. Env: setting `FORGE_SFTP_LISTEN` also enables it |
| `sftp.listen` | `0.0.0.0:2022` | SFTP listener. Env: `FORGE_SFTP_LISTEN` |
| `sftp.public_host` | host of `public_url` | Host name shown to users with each access |
| `sftp.public_port` | port of `sftp.listen` | Port shown to users, if a proxy or NAT maps another one |
| `sftp.max_hours` | `168` | Longest lifetime of an access, in hours |
| `sftp.allow_never_expiry` | `false` | Allow access that never expires. Even then, only users with `file.admin` on the node (**Admin**, **Platform admin**) may choose it. When `false`, `max_hours` caps every access |

```toml
[sftp]
enabled = true
listen = "0.0.0.0:2022"
public_host = "files.example.com"
max_hours = 72
```

The host key is created on first start at `<data_dir>/keys/sftp_host_ed25519`
(mode `0600`). Delete it and restart to rotate it.

:::caution
With `allow_never_expiry = true`, a leaked password works until someone
revokes the access. Turn it on only for automated transfers you track, and
pair each such access with allowed addresses and a transfer allowance.
Access that never expires is flagged in the console and recorded in the
audit log (`never_expires: true`). Turning the setting off later does not
change access that already exists; revoke it or set an expiry.
:::

### MCP connectors

The `[mcp]` section serves the [MCP endpoint](/docs/reference/mcp/) at
`<public_url>/mcp` for AI agents, with OAuth discovery, client registration
and the consent page. It is off by default. See
[Connect AI agents](/docs/guides/connect-ai-agents/).

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `mcp.enabled` | boolean | `false` | Serve `/mcp`, `/.well-known/oauth-protected-resource`, `/oauth/register` and the consent page. Listing and revoking connections work either way. Env: `FORGE_MCP_ENABLED=1` (or `true`) |
| `mcp.max_mode` | string | `"auto"` | Highest mode a person may give a connector: `read_only`, `ask` or `auto`. The consent page defaults to `ask`, or to `max_mode` when that is lower. Lowering it also limits existing connections on their next call. Env: `FORGE_MCP_MAX_MODE`; any other value stops Forge from starting |
| `mcp.registrations_per_hour` | integer | `30` | Dynamic client registrations accepted per rolling hour, across all callers |

```toml
public_url = "https://forge.example.com"

[mcp]
enabled = true
max_mode = "read_only"
registrations_per_hour = 30
```

`public_url` must be the HTTPS URL agents use: it is the OAuth issuer, the
resource identifier and the only allowed `Origin`.

### Port tunnels

The `[tunnels]` section limits [port tunnels](/docs/concepts/port-tunnels/) and
sets their lifetimes.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `tunnels.max_per_user` | integer | `8` | Open tunnels one user may have. One more answers `409` |
| `tunnels.max_connections_per_node` | integer | `32` | Tunnel connections to one node at once. The agent also refuses more than 32 |
| `tunnels.max_connections` | integer | `256` | Tunnel connections on this server at once |
| `tunnels.idle_timeout_secs` | integer | `900` | A connection, or a tunnel without connections, closes after this long without traffic. 60 to 86400 |
| `tunnels.default_lifetime_hours` | integer | `8` | Lifetime of a tunnel (or grant) opened without one. 1 to `max_lifetime_hours` |
| `tunnels.max_lifetime_hours` | integer | `168` | Longest lifetime a tunnel or a tunnel grant may ask for; a longer request answers `400`. 1 to 168. A grant's own expiry caps tunnels opened through it |
| `tunnels.allow_never_expiry` | boolean | `false` | Allow tunnels and grants that never expire. Even then, only callers with `access.manage` on the workspace (**Admin**, **Platform admin**, MFA) may choose it (`--for never`, `never_expires: true`). When `false`, a `never` request answers `403`, audited as a denied `network.tunnel.open` or `network.tunnel.grant.create` |

```toml
[tunnels]
max_per_user = 8
default_lifetime_hours = 8
max_lifetime_hours = 168
```

Which ports a tunnel may reach is not in this file: the server lists the
ports of every service from the stored app specs, compose stacks, App Store
packages and SSH endpoints (the [service port catalogue](/docs/reference/rest-api/#service-port-catalogue)),
and a tunnel must name one of them. Only administrators with
`network.tunnel.any` may open a raw port.

:::caution
A never-expiring tunnel grant is standing access: the person can open the
tunnel at any time until someone revokes the grant. Allow it only where the
person would otherwise hold a role on the environment anyway, and review
**Administration ▸ Tunnels ▸ Grants** regularly.
:::

:::note
A `[remote_access]` section from before WireGuard remote access was removed
is accepted and ignored; the server logs a warning at start. Delete it.
:::

### Alerts

The `[alerts]` section limits [alert rules](/docs/concepts/alerts/) and sets
where and how [alarm notifications](/docs/concepts/alerts/#notifications) are
sent. These keys have no environment variables. A change takes effect when
the server restarts.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `alerts.max_rules_per_workspace` | integer | `200` | Most alert rules one workspace may have, 1 to 10000. Each evaluation pass costs rules × watched nodes, every 15 seconds. Creating one more answers `409` |
| `alerts.webhook_allow_http` | boolean | `false` | Allow `http://` URLs for notification channels, and email channels with `tls = none`. Messages and the URL's token then travel unencrypted. When `false`, only `https://` and encrypted SMTP (STARTTLS or implicit TLS) are accepted. An SMTP user name is never sent without TLS, whatever this setting says |
| `alerts.webhook_allow_private` | boolean | `false` | Allow channels that target private networks: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `100.64.0.0/10`, `198.18.0.0/15`, `fc00::/7` and `fec0::/10`. Link-local, cloud metadata, multicast and reserved addresses stay refused |
| `alerts.webhook_allow_loopback` | boolean | `false` | Allow channels that target the server's own loopback interface (`127.0.0.0/8`, `::1`, `localhost`). For tests and single-host labs |
| `alerts.delivery_attempts` | integer | `6` | Attempts per notification before it is recorded as `failed`, 1 to 20 |
| `alerts.delivery_retry_base_secs` | integer, seconds | `30` | Ceiling of the wait before the first retry, 1 to 3600. The ceiling doubles after each attempt; the actual wait is between half the ceiling and the ceiling |
| `alerts.delivery_retry_max_secs` | integer, seconds | `900` | Longest wait between two attempts, from `delivery_retry_base_secs` to 86400 |

```toml
[alerts]
max_rules_per_workspace = 200
webhook_allow_private = false
delivery_attempts = 6
delivery_retry_base_secs = 30
delivery_retry_max_secs = 900
```

The three `webhook_allow_*` settings apply to webhook, Slack and email
channels alike, when a channel is saved and again on every connection. Turning one off blocks existing channels that
needed it: their deliveries fail with `blocked_target`. A value outside its
range stops the server from starting. See the
[retry schedule](/docs/reference/notification-payload/#retry-schedule).

:::caution
`webhook_allow_private` and `webhook_allow_loopback` let anyone with
`notification.manage` make the control plane send `POST` requests to
services on your internal network or on the Forge host itself. Turn them on
only when your receiver is there, and keep channel management with people
you trust on that network.
:::

### Local names

The `[names]` section controls the [local names](/docs/concepts/local-names/)
every node keeps in its hosts file.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `names.enabled` | boolean | `true` | Send every enrolled node the names of its workspace's nodes and services. Turning it off stops updates; blocks already written stay until a node is sent an empty list |
| `names.suffix` | string | `"local"` | Suffix of every name, unless a platform admin set the runtime setting (see below). Lowercase DNS labels separated by dots, at most 63 characters. One label from `local`, `internal`, `lan`, `home`, `test`, `localdomain`, `intranet` or `corp`, or a domain of two or more labels that you own (`corp.example.com`). Public top-level domains (`com`, `io`, …) and anything with `localhost` stop Forge from starting. Env: `FORGE_NAMES_SUFFIX` |
| `names.containers` | boolean | `true` | Also write the names into the hosts files of each node's Forge containers (label `forge.app`) |

The suffix in use is the first that is set: the runtime setting
`names.suffix` (set with `PUT /api/v1/names/suffix`, `forge dns suffix set`
or the console, kept in the settings store, no restart), then
`names.suffix` in this file or `FORGE_NAMES_SUFFIX`, then `local`. The
generic settings editor (`PUT /api/v1/settings/{key}`) refuses keys under
`names.`. See [The suffix](/docs/concepts/local-names/#the-suffix).

```toml
[names]
suffix = "local"
containers = true
```

:::caution
`.local` is also the mDNS domain (RFC 6762). Hosts-file entries win on
Debian, Ubuntu and RHEL-likes because `nsswitch.conf` lists `files` first,
but programs that query mDNS directly (Avahi tools, some browsers) do not
read the hosts file. See [the trade-offs](/docs/concepts/local-names/#local-and-mdns).
:::

## Environment overrides

`FORGE_DATA_DIR`, `FORGE_DATABASE_URL`, `FORGE_API_LISTEN`,
`FORGE_AGENT_LISTEN`, `FORGE_PUBLIC_URL`, `FORGE_COOKIE_SECURE`,
`FORGE_ADMIN_PASSWORD` (for `init` only), `FORGE_ENVIRONMENT`,
`FORGE_REQUIRE_MFA`, `FORGE_LOGIN_CHALLENGE`, `FORGE_DEV_SKIP_MFA`
(deprecated), `FORGE_STORAGE_ALLOW_LOOPBACK`, `FORGE_SFTP_LISTEN`,
`FORGE_MCP_ENABLED`, `FORGE_MCP_MAX_MODE`, `RUST_LOG`.

An environment variable wins over the file. `forge-server mfa status` says so
when `FORGE_REQUIRE_MFA` or `FORGE_DEV_SKIP_MFA` overrides `require_mfa`.

## Agent (`agent.toml`)

The file lives in the agent's state directory:
`/var/lib/forge-agent/agent.toml` on Linux and
`%ProgramData%\OrbynForge\agent\agent.toml` on Windows. Unknown keys are
rejected.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `server_url` | string | required | `https://host:port` of the agent listener |
| `allowed_commands` | string array | see below | Local allowlist of command names. The agent refuses any other command, even when it is correctly signed |
| `offered_commands` | string array | written by the agent | Command names an install or update has already offered. Managed by Forge; do not edit. A name listed here but missing from `allowed_commands` was removed on purpose and stays removed |
| `analytics_url` | string | written by the agent | `https://host:port` of the analytics worker's upload listener, from enrollment. Heartbeat answers keep it current; do not edit. Missing when analytics was off at enrollment |

Default `allowed_commands`: `ping`, `collect_facts`, `service_status`,
`package`, `container`, `app`, `site`, `dns`, `probe`, `ingress`, `fabric`,
`storage`, `files`, `access`, `cancel`, `processes`, `process_action`,
`update`, `stack`, `terminal_account`, `names`, `tunnel`, `container_exec`,
`docker_tls`, `swarm`. Agents enrolled
before port tunnels replaced remote access may still list `access`; the
agent ignores names it does not know.

| Command | What removing it turns off on the node |
| --- | --- |
| `files` | The file explorer |
| `terminal_account` | Creating terminal accounts from Forge on an enrolled node (the node installer creates them as root and is not affected) |
| `swarm` | Creating, joining and leaving a Docker Swarm, and changing swarm nodes, from a [Swarm cluster](/docs/concepts/clusters/#cluster-modes) |
| `process_action` | Ending processes and changing their priority from the **Processes** panel, `forge nodes kill` and `forge nodes priority`. The process list (`processes`) keeps working |
| `tunnel` | [Port tunnels](/docs/concepts/port-tunnels/) to this node |
| `container_exec` | [Container shells](/docs/concepts/container-shells/) through the agent. **Opt-in on existing agents:** an update or `offer-commands` never adds it; add `"container_exec"` to `allowed_commands` and restart the agent. New enrollments have it. `container` does not cover it |
| `names` | Writing the fleet's [local names](/docs/concepts/local-names/) into the hosts file. Without it the node keeps the last names it wrote and other nodes still resolve it |
| `cancel` | Stopping a running recipe or install when its job is cancelled; the operation then runs to the end |

:::note
Enrollment writes the full list into `agent.toml`. Re-running the node
installer adds only command names that are new in that release
(`forge-agent offer-commands`); names you removed stay removed. To allow a
removed name again, add it to `allowed_commands` and restart the agent.
:::

```toml
server_url = "https://<forge-host>:7443"
allowed_commands = ["ping", "collect_facts", "service_status", "package", "container", "app", "site", "dns", "probe", "ingress", "fabric", "storage", "files", "access", "cancel", "processes", "process_action", "update", "stack", "terminal_account", "names", "tunnel", "container_exec", "docker_tls", "swarm"]
```

## Databases

Forge uses two databases: the **operational** database for users, access,
nodes, apps and jobs, and an optional **analytics** database for node
history and logs. See [Databases](/docs/guides/databases/).

### `[database]`

| Key | Default | Description |
| --- | --- | --- |
| `url` | none (SQLite in `data_dir`) | PostgreSQL server address without the database name and without a password, for example `postgres://forge@db1:5432`. A password in the URL is refused |
| `name` | `forge_op` | Database name on that server. Letters, digits and `_`. Created on first use when missing; an existing database is reused, never recreated |
| `password_file` | none | File with the password on its first line (mode `0600`, owned by `forge`). Env `FORGE_DATABASE_PASSWORD` overrides it |

```toml
[database]
url = "postgres://forge@db1:5432"
name = "forge_op"
password_file = "/etc/forge/db-password"
```

### `[analytics]`

| Key | Default | Description |
| --- | --- | --- |
| `engine` | `duckdb` | `off`, `embedded` (the ClickHouse engine run by Forge on `127.0.0.1`, Linux only; `chdb` is an alias), `clickhouse` (a ClickHouse server or ClickHouse Cloud) or `duckdb` (the default: DuckDB inside the analytics worker, one database file per day under `<data_dir>/analytics/duckdb`; for installs up to about 100 nodes with moderate logging; see [Benchmarks](/docs/development/benchmarks/#analytics-duckdb-against-chdb)). `memory` keeps rows in the analytics worker's memory only, for tests and trials; nothing survives a restart. `postgres` is reserved: it is accepted, logs a warning and leaves analytics off. Any engine but `off` starts the analytics worker |
| `url` | none | `clickhouse`: the HTTP(S) address without a password, for example `https://default@ch1:8443`. The user defaults to `default` |
| `name` | `<database.name>_analytics` | Analytics database name, for example `forge_op_analytics` |
| `password_file` | none | Password file for `clickhouse`; env `FORGE_ANALYTICS_PASSWORD` overrides it. `embedded` generates its own password in `<data_dir>/analytics/secret` |
| `embedded_port` | `18123` | `embedded`: loopback HTTP port of the engine |
| `embedded_memory_mb` | `1024` | `embedded`: the engine's memory cap (`max_server_memory_usage`), MiB, 256–1048576 (setting `memory_budget_mb`). The engine enforces it: work that would exceed it fails |
| `duckdb_memory_mb` | `256` | `duckdb`: DuckDB's `memory_limit`, MiB, 256–1048576 (setting `memory_budget_mb`). Work beyond it spills to `<data_dir>/analytics/duckdb/tmp` or fails. `worker_memory_limit_mb` must be at least this plus 256, or the server does not start |
| `embedded_binary` | `<data_dir>/analytics/bin/clickhouse` | `embedded`: the engine binary, installed with `forge-server analytics install` |
| `worker_memory_limit_mb` | `2048` | Resident memory of the analytics worker process, MiB, above which `forge-server` restarts it. `0` is no limit; otherwise at least `128`. Does not include the embedded engine, which has its own budget (`embedded_memory_mb`). With `duckdb` it does include DuckDB, which runs inside the worker |
| `worker_binary` | `forge-analytics-worker` next to `forge-server` | Path of the analytics worker binary |
| `worker_agent_listen` | `0.0.0.0:55132` | Address where the analytics worker accepts spool uploads from agents (HTTP/2 over mTLS). Nodes must reach this port; see [Network ports](/docs/reference/network-ports/) |
| `worker_agent_url` | `https://<host of agent_public_url>:<port of worker_agent_listen>` | URL agents upload spool files to, sent at enrollment and in every heartbeat answer. Its host must be one of `agent_hostnames`, which the worker's certificate names. Set it when nodes reach the worker under another name or port, for example through NAT |
| `worker_max_uploads` | `32` with `duckdb`, else `8` | Spool files the worker imports at once, `8` to `32`; with `duckdb`, every file waiting is written in one batch, so more slots mean larger batches; other values stop the server at startup. Each slot is one agent upload in flight, so a higher value stores more rows per second and uses more network. One more upload answers `429` at once; the agent backs off and retries, and the file waits in its spool. On a 2-CPU engine, 4 slots stored about 4,000 rows/s and 32 about 10,000 rows/s (see `scripts/dev/benchmarks/2026-11-21-chdb-capped-node-scaling.md`) |
| `cpu_quota_vcpu` | `0` | `embedded`: CPU cap of the engine, vCPUs (decimal). `0` is no cap; otherwise 0.1–256. Rounded up to whole CPUs: the engine is pinned to them (CPU affinity) and its thread pools are sized to match. Linux only. `duckdb`: DuckDB's `threads`, rounded up; `0` uses 2 threads |
| `memory_limit_mb` | none | Removed. Still accepted so an older `forge.toml` loads, but ignored with a warning: `embedded_memory_mb` caps the engine's memory |
| `min_free_disk_mb` | `1024` | Log ingest answers `503 ingest_disk_low` while the engine has less free disk, MiB, 0–10485760 |
| `retention.node_metrics_days` | `7` | Days of per-minute node metrics and container metrics (`container_metrics_1m`), 1–3650 |
| `retention.top_processes_days` | `node_metrics_days` | Days of per-minute top processes, 1–3650 |
| `retention.node_rollup_days` | `365` | Days of hourly node trends and container metrics (`container_metrics_1h`), 1–3650 |
| `retention.app_logs_days` | `30` | Days of service logs, 1–3650, counted from when Forge stored each event (`ingest_ts`), not from the event's own time |
| `search.max_concurrent` | `8` | Log searches running at once, 1–256; more answer `503 search_busy` |
| `search.timeout_secs` | `20` | Seconds one log search may run, 1–3600 |
| `search.max_lookback_days` | `0` | How far back a log search may reach, days, 0–3650. `0` is no limit; an earlier start is moved forward to the limit |
| `search.threads` | `0` | Threads per log search, 0–256. `0` keeps the engine default |
| `search.memory_mb` | `0` | Memory per log search, MiB, 0–1048576. `0` keeps the engine default |
| `ingest.max_queued_mb` | `256` | Parsed log events waiting to be stored before ingest answers `503 ingest_busy`, MiB, 1–65536. Counted as the events' size in the worker's memory, not as received |
| `ingest.max_batch_rows` | `20000` | Rows in one insert, 100–1000000 |
| `ingest.flush_interval_ms` | `100` | How long the writer waits for more batches before an insert, ms, 10–10000 |

Every key from `cpu_quota_vcpu` down is a default. A platform administrator
can override each one at runtime under **Administration → Settings →
Analytics** or with `PUT /api/v1/analytics/settings`, without a restart. The
overrides are stored in the operational database (settings key
`analytics.overrides`) and win over `forge.toml`; removing an override goes
back to the `forge.toml` value. `embedded_memory_mb` and `duckdb_memory_mb`
are overridable the same way (setting `memory_budget_mb`). With
`engine = "duckdb"`, a change to `cpu_quota_vcpu` or `memory_budget_mb`
applies to DuckDB at once. With `engine = "clickhouse"`, retention is applied to the external
server; CPU and memory belong to that host, and Forge refuses changes to
`cpu_quota_vcpu` and `memory_budget_mb`. See
[Configure analytics retention and limits](/docs/guides/analytics-settings/)
and [REST API: Analytics settings](/docs/reference/rest-api/#analytics-settings).

With `duckdb`, `search.threads` and `search.memory_mb` have no effect:
DuckDB's threads and memory limit belong to the whole instance, not to one
query. `search.timeout_secs` interrupts a search that runs too long.

A small install on DuckDB:

```toml
[analytics]
engine = "duckdb"
duckdb_memory_mb = 256
cpu_quota_vcpu = 2.0
```

A larger install on the embedded engine:

```toml
[analytics]
engine = "embedded"
worker_memory_limit_mb = 1024
cpu_quota_vcpu = 2.0
embedded_memory_mb = 2048
min_free_disk_mb = 2048

[analytics.retention]
node_metrics_days = 14
top_processes_days = 7
app_logs_days = 30

[analytics.search]
max_concurrent = 4
timeout_secs = 30
max_lookback_days = 14

[analytics.ingest]
max_queued_mb = 512

[analytics.spool]
max_mb = 1024
max_age_hours = 48

[analytics.listener]
rate_per_sec = 200
```

#### `[analytics.spool]`

The spool every agent keeps on its node (see
[Analytics: node spools](/docs/concepts/analytics/#node-spools)). The server
sends these values to every node in heartbeat answers, so a change applies
to the whole fleet after a `forge-server` restart, within one heartbeat.
`0` keeps the agent's default. The agent clamps each value to the range
shown.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `spool.max_mb` | integer, MiB | `0` (agent: `512`) | Most disk the spool uses, sealed files plus the write buffer. Clamped to 16–65536. At the cap the oldest file is dropped and counted |
| `spool.min_free_mb` | integer, MiB | `0` (agent: `256`) | Nothing is written while the spool's disk has less free space. The local listener answers `429` |
| `spool.max_age_hours` | integer, hours | `0` (agent: `72`) | Sealed files older than this are dropped. Clamped to 10 minutes–30 days |
| `spool.seal_kb` | integer, KiB | `0` (agent: `4096`) | The write buffer is sealed into a file at this many raw bytes. Clamped to 64–16384 |
| `spool.seal_secs` | integer, seconds | `0` (agent: `10`) | Or the write buffer is sealed this long after its first row, whichever comes first. Clamped to 1–300. Also the most application log data a node crash can lose |
| `spool.zstd_level` | integer | `0` (agent: `1`) | zstd level, `0` or 1–3. Any other value stops `forge-server` from starting |

#### `[analytics.listener]`

The local log listener on every node (`127.0.0.1:5341` and the Docker
bridge gateway). Sent to nodes like `[analytics.spool]`.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `listener.max_body_kb` | integer, KiB | `0` (agent: `1024`) | Largest request body. Larger answers `413`. Clamped to 1–16384 |
| `listener.rate_per_sec` | integer | `0` (agent: `50`) | Requests per second per ingest key (burst twice that), at most 100000. Over the rate answers `429` |
| `listener.rate_on_loopback` | boolean | `false` | Apply the rate on `127.0.0.1` too. By default only requests on the Docker bridge address are rated |

The spool and listener keys are not runtime settings: edit `forge.toml` and
restart `forge-server`.

