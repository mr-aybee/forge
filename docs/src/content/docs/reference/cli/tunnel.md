---
title: forge tunnel
description: Forward a local port to a service, a stack service, a database or SSH on a node; list and close tunnels; manage remembered local ports and explicit grants.
sidebar:
  order: 21
---

`forge tunnel` forwards a port on your computer to a service on a node, like
`ssh -L`, through the Forge API port. The API port is the only way in: there
is no VPN and no open port on the node. Targets are **services**: a
Forge service, a service of a compose stack, a database from the App Store, or
a node's SSH endpoint. A raw port on a node is the administrators'
exception. For how it works, see [Port tunnels](/docs/concepts/port-tunnels/);
for the task, see [Open a port tunnel](/docs/guides/port-tunnels/). Every
command takes the [global flags](/docs/reference/cli/#global-flags).

Who may open a tunnel: a **Developer**, **Operator** or **Admin** role on
the service's environment (`network.tunnel`), or an explicit
[grant](#forge-tunnel-grants-create) from an administrator. SSH targets
need `terminal.open` on the node. A raw `--node --port` needs
`network.tunnel.any` (**Workspace admin**, **Platform admin**). All of
them need an MFA-verified session; on a terminal the CLI asks for your code.

The sample output below comes from a lab with the nodes `lab-b` and `lab-c`.

## Commands

| Command | What it does |
| --- | --- |
| [`forge tunnel open`](#forge-tunnel-open) | Forwards a local port to a service, a database or SSH, until Ctrl-C |
| [`forge tunnel list`](#forge-tunnel-list) | Lists your open tunnels |
| [`forge tunnel close`](#forge-tunnel-close) | Closes a tunnel and its connections |
| [`forge tunnel ports`](#forge-tunnel-ports) | Lists the local ports this machine remembers per target |
| [`forge tunnel ports forget`](#forge-tunnel-ports-forget) | Forgets a remembered local port |
| [`forge tunnel grants list`](#forge-tunnel-grants-list) | Lists explicit tunnel grants |
| [`forge tunnel grants create`](#forge-tunnel-grants-create) | Grants one person access to a service, a database, SSH or a raw port |
| [`forge tunnel grants revoke`](#forge-tunnel-grants-revoke) | Revokes a grant and closes its tunnels |

Exit codes for every command: `0` success, `1` the local port could not be
opened or the prompt was cancelled, `2` invalid flags or a choice the CLI
could not make for you, `3` not signed in, `4` the target is not allowed
for you, `5` a second factor is needed, `6` no such service, node, tunnel or
grant, `7` a tunnel limit is reached, `9` the server is unreachable.

### Targets

`open` and `grants create` take the same target flags. Name one target:

| Target | Flags | Resolves to |
| --- | --- | --- |
| Service | `--app <NAME>` | The app's listening port on the nodes it runs on |
| Stack service | `--stack <NAME> --service <NAME>` | The service's published ports on the node that runs it |
| Database | `--db <NAME>` | The package installation's port on its node |
| Any service | `--service <NAME>` | Whatever has that name; the CLI asks you to narrow it when two kinds match |
| SSH | `--node <NODE> --ssh` | The node's configured SSH port |
| Raw port | `--node <NODE> --port <PORT>` | That port on the node (administrators only, audited as raw) |

The CLI resolves a service with the server's port catalogue
(`GET /api/v1/service-ports`, see the [REST API](/docs/reference/rest-api/)) and
sends the server a target `{kind, ref, server_id, ports}`; the server
checks your role or grant for every node and port.

### forge tunnel open

Resolves the target, listens on `--bind:<local port>` and forwards every
connection to the service's port on the node. It runs in the foreground
until you press Ctrl-C or the server closes the tunnel (closed by someone,
idle, lifetime over, grant revoked, or your sign-in ended). Audited as
`network.tunnel.open`.

**Which node.** An app can run on several nodes: without `--node` the CLI
takes the first node that is online. **Which port.** A service that listens
on several ports (a stack service with a UI and an ingest port, say) needs a
choice: `--port <PORT>` picks one, `--all` opens one tunnel per port. On a
terminal the CLI asks; with `--non-interactive`, a pipe or `-o json` it
stops with exit code `2` and lists the ports.

**Which local port.** `--local-port` if you pass it; else the port this
machine remembers for the same target, node and remote port; else a free
port, which the CLI then remembers in your context (see
[`forge tunnel ports`](#forge-tunnel-ports)). The same service lands on the
same local port next time, so bookmarks and client configs keep working.

**Synopsis**

```text
forge tunnel open (--app <NAME> | --stack <NAME> --service <NAME> | --service <NAME> | --db <NAME> | --node <NODE> --ssh | --node <NODE> --port <PORT>)
                  [--node <NODE>] [--port <PORT> | --all] [--environment <ENVIRONMENT>]
                  [--local-port <LOCAL_PORT>] [--bind <BIND>] [--allow-remote-bind] [--for <DURATION>]
                  [--direct]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--app <NAME>` | service name | — | one target | A service | `--app site` |
| `--stack <NAME>` | stack name | — | one target, with `--service` | A compose stack; `--service` names the service in it | `--stack seq-b --service seq` |
| `--service <NAME>` | service name | — | one target | A service of any kind. With `--stack`, the service in that stack. Alone, the CLI asks you to narrow it when the name exists as several kinds or in several environments | `--service seq` |
| `--db <NAME>` | package installation name | — | one target | A database from the App Store | `--db orders-pg` |
| `--node <NODE>`, `-n` | node id, host name or display name | the first online node the service runs on | for `--ssh` and a raw port | The node. For a service: one of the nodes it runs on | `-n lab-c` |
| `--ssh` | flag | off | one target, with `--node` | The node's SSH port (`forge nodes ssh` shows it) | `--node lab-b --ssh` |
| `--port <PORT>`, `-p` | integer 1–65535 | the service's only port | no | One port of a service that listens on several. With `--node` and no service: a raw port on the node (`network.tunnel.any`) | `-p 80` |
| `--all` | flag | off | no | Every port of the service, one tunnel each. Conflicts with `--port` and `--local-port` | `--all` |
| `--environment <ENVIRONMENT>` | environment name or id | — | no | The service's environment, when the same name exists in more than one | `--environment web` |
| `--local-port <LOCAL_PORT>`, `-l` | integer 1–65535 | remembered port, else a free one | no | The port to listen on here | `-l 3000` |
| `--bind <BIND>` | IP address | `127.0.0.1` | no | The address to listen on. Another address than loopback needs `--allow-remote-bind` | `--bind ::1` |
| `--allow-remote-bind` | flag | off | no | Allow a `--bind` other machines can reach. Anyone who can reach it can use the tunnel; the CLI warns and the bind is audited | `--allow-remote-bind` |
| `--for <DURATION>` | `<n>m`, `<n>h`, `<n>d` (combinable: `1h30m`) or `never` | `8h` | no | How long the tunnel may stay open; at most the server's `[tunnels] max_lifetime_hours` (7 days by default). `never` needs `[tunnels] allow_never_expiry = true` and an admin role | `--for 3d` |
| `--direct` | flag | off | no | Connect straight to the node's SSH port instead of relaying through the Forge server. Each connection gets a one-minute certificate that may only forward to this target. Needs the node's SSH endpoint (pinned host key) reachable from your machine, and a node set up for the browser terminal with a current agent | `--direct` |

:::caution
`--for never` and raw ports bypass the limits that protect you from
forgotten tunnels. Prefer a service target with a lifetime; every tunnel
and every denial is in the audit log.
:::

**Examples**

```sh
# A service; the CLI picks a node, its port and a local port
forge tunnel open --app site

# The same app on a chosen node, on a chosen local port
forge tunnel open --app site --node lab-c -l 3000

# A stack service with several ports: pick one, or open them all
forge tunnel open --stack seq-b --service seq --port 80
forge tunnel open --stack seq-b --service seq --all

# A database, then connect from another terminal
forge tunnel open --db orders-pg
psql -h 127.0.0.1 -p 15432 -U postgres

# SSH to a node
forge tunnel open --node lab-b --ssh

# A raw port (administrators)
forge tunnel open --node lab-b --port 9000 --for 30m

# Straight to the node's SSH, not through the server
forge tunnel open --app web-html --node lab-c -l 18081 --direct
```

**Output**

The first line says what the service is and where it runs. Each tunnel then
gets an arrow line: the node and remote port, the local side, and when it
expires. The local side is a URL when the port's role is `http`.
`(remembered)` marks a local port the CLI took from your context.

```text
$ forge tunnel open --app site
site (web) runs on lab-b, lab-c · listens on 8080
→ lab-b:8080  →  http://127.0.0.1:18080   (expires in 8 h, Ctrl-C to close)
```

```text
$ forge tunnel open --app site
site (web) runs on lab-b, lab-c · listens on 8080
→ lab-b:8080  →  http://127.0.0.1:18080 (remembered)   (expires in 8 h, Ctrl-C to close)
```

With `--direct`, the arrow line says `direct SSH`:

```text
$ forge tunnel open --app web-html --node lab-c -l 18081 --direct
web-html (demo) runs on lab-c · listens on 8081
→ lab-c:8081  →  http://127.0.0.1:18081   (direct SSH, expires in 8 h, Ctrl-C to close)
```

A node that cannot take direct tunnels:

```text
$ forge tunnel open --app web-html --node lab-c --direct
web-html (demo) runs on lab-c · listens on 8081
error: this node has no SSH endpoint with a pinned host key; set one on the node's Terminal page [conflict]
```

A stack service with several ports, on a terminal:

```text
$ forge tunnel open --stack seq-b --service seq
seq-b/seq (web) runs on lab-c · listens on 80, 5341
seq-b/seq (web) listens on several ports on lab-c:
  1) 80 (http)
  2) 5341 (ingest)
Port to open [1-2]: 1
→ lab-c:80  →  http://127.0.0.1:18180   (expires in 8 h, Ctrl-C to close)
```

The same without a terminal:

```text
$ forge tunnel open --stack seq-b --service seq --non-interactive
error: seq-b/seq (web) listens on 80 (http), 5341 (ingest) on lab-c; pick one with --port, or open every port with --all [invalid_input]
```

SSH prints the command to paste; a database prints a client hint when the
engine is known (PostgreSQL, MySQL/MariaDB, Redis, SQL Server, MongoDB):

```text
$ forge tunnel open --node lab-b --ssh
lab-b · SSH on 22
→ lab-b:22  →  127.0.0.1:18022   (expires in 8 h, Ctrl-C to close)
  ssh -p 18022 forge-ops@127.0.0.1
```

```text
$ forge tunnel open --db orders-pg
orders-pg (web) runs on lab-c · listens on 5432
→ lab-c:5432  →  127.0.0.1:15432   (expires in 8 h, Ctrl-C to close)
  psql -h 127.0.0.1 -p 15432 -U postgres
```

When the tunnel ends, a last line names the reason: `closed here`
(Ctrl-C), `closed by the server (closed)`, `(idle)`, `(expired)`,
`(grant_revoked)`, `(access_withdrawn)` or `(server_restart)`:

```text
Tunnel 765e21345c4863d054354e2e5eac428e: closed by the server (expired)
```

A target you may not reach:

```text
$ forge tunnel open --app site
error: you have no role on environment web and no grant for app site; ask an administrator for `forge tunnel grants create` [permission_denied]
```

With `-o json` the CLI prints one object (or an array with `--all`) instead
of the first lines, then holds the tunnel:

```json
{
  "result": {
    "expires_at": 1794583393472,
    "id": "765e21345c4863d054354e2e5eac428e",
    "local": "127.0.0.1:18080",
    "node": "lab-b",
    "port": 8080,
    "remembered": false,
    "role": "http",
    "server_id": "01900000-0000-7000-8000-000000000006",
    "target": { "kind": "app", "ports": [8080], "ref": "site", "server_id": "01900000-0000-7000-8000-000000000006" },
    "url": "http://127.0.0.1:18080"
  },
  "schema": "forge.cli/v1"
}
```

### forge tunnel list

Lists your open tunnels. Needs nothing beyond being signed in; `--all`
needs `user.manage`.

**Synopsis**

```text
forge tunnel list [--all]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--all` | flag | off | no | Everyone's tunnels | `--all` |

**Examples**

```sh
# Your open tunnels
forge tunnel list
# Everyone's tunnels (needs user.manage)
forge tunnel list --all
```

**Output**

```text
$ forge tunnel list
ID                                TARGET    NODE   PORT  BIND             CONNECTIONS  UP      DOWN     EXPIRES  USER
765e21345c4863d054354e2e5eac428e  app site  lab-b  8080  127.0.0.1:18080  1            9.1 kB  41.2 kB  in 8 h   dev@example.com
```

`UP` is traffic to the node, `DOWN` from it. With `-o json`, each tunnel has
the [REST tunnel fields](/docs/reference/rest-api/) plus `node` (the node's
name) and the display columns.

### forge tunnel close

Closes a tunnel: every connection ends at once and the `forge tunnel open`
running it exits. Your own tunnels, or anyone's with `user.manage`. Audited
as `network.tunnel.revoke`.

**Synopsis**

```text
forge tunnel close <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | tunnel id | — | yes | From `forge tunnel list`. `--id <ID>` still works for older scripts | `765e21345c4863d054354e2e5eac428e` |

**Examples**

```sh
# Close one tunnel
forge tunnel close <tunnel-id>
```

**Output**

```text
$ forge tunnel close 765e21345c4863d054354e2e5eac428e
{
  "closed": "765e21345c4863d054354e2e5eac428e"
}
```

### forge tunnel ports

Lists the local ports this machine remembers for tunnel targets, from the
current context's file. A key reads `<kind>:<target>@<node>:<remote port>`.
Local only; needs no permission and does not contact the server.

**Synopsis**

```text
forge tunnel ports
```

**Examples**

```sh
# Remembered local ports
forge tunnel ports
# As JSON
forge -o json tunnel ports
```

**Output**

```text
$ forge tunnel ports
TARGET             NODE   REMOTE  LOCAL  KEY
app site           lab-b  8080    18080  app:site@lab-b:8080
package orders-pg  lab-c  5432    15432  package:orders-pg@lab-c:5432
ssh                lab-b  22      18022  ssh:lab-b@lab-b:22
stack seq-b/seq    lab-c  80      18180  stack:seq-b/seq@lab-c:80
```

With `-o json` each row is `{key, kind, target, node, remote, local}`.

### forge tunnel ports forget

Forgets remembered local ports. The next `forge tunnel open` for that
target picks a fresh free port. Local only.

**Synopsis**

```text
forge tunnel ports forget <TARGET>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<target>` | key, target name or node name | — | yes | A whole key as `forge tunnel ports` shows it, or a target name (`site`, `seq-b/seq`) or node name, which forgets every mapping of that target or node | `site` |

**Examples**

```sh
# Forget every mapping of one target
forge tunnel ports forget site
# Forget one exact mapping
forge tunnel ports forget app:site@lab-b:8080
```

**Output**

```text
$ forge tunnel ports forget site
{
  "forgotten": [
    "app:site@lab-b:8080"
  ]
}
```

Exit code `6` when nothing matches.

### forge tunnel grants list

Lists explicit tunnel grants: who may reach which service, database, SSH
endpoint or raw port, until when. Active grants by default. Needs
`access.manage`.

**Synopsis**

```text
forge tunnel grants list [--all]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--all` | flag | off | no | Expired and revoked grants too | `--all` |

**Examples**

```sh
# Your tunnel grants
forge tunnel grants list
# Everyone's grants
forge tunnel grants list --all
```

**Output**

```text
$ forge tunnel grants list
ID                                    USER             TARGET    NODE   PORTS  EXPIRES  STATUS  NOTE
01900000-0000-7000-8000-000000000041  dev@example.com  app site  any    *      in 1 h   active  on-call review
01900000-0000-7000-8000-000000000042  ops@example.com  ssh       lab-b  22     in 2 d   active
```

`NODE` is `any` and `PORTS` is `*` when the grant covers every node or port
of the service. With `-o json`, each grant has the REST fields: `id`,
`user_id`, `target` (`{kind, ref, server_id, ports}`), `created_by`,
`created_at`, `expires_at` (`null` when it never expires), `revoked_at`,
`note`.

### forge tunnel grants create

Grants one person access to a target they have no environment role for. The
grant has its own expiry; tunnels opened through it close when it expires
or is revoked. Needs `access.manage`; audited as
`network.tunnel.grant.create`. `--for never` also needs
`[tunnels] allow_never_expiry = true` on the server.

**Synopsis**

```text
forge tunnel grants create --user <USER> (--app <NAME> | --stack <NAME> --service <NAME> | --service <NAME> | --db <NAME> | --node <NODE> --ssh | --node <NODE> --port <PORT>)
                           [--node <NODE>] [--port <PORT>] [--environment <ENVIRONMENT>] [--for <DURATION>] [--note <NOTE>]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--user <USER>` | username, email or user id | — | yes | The person | `--user dev@example.com` |
| `--app <NAME>` | service name | — | one target | A service | `--app site` |
| `--stack <NAME>` | stack name | — | one target, with `--service` | A compose stack | `--stack seq-b --service seq` |
| `--service <NAME>` | service name | — | one target | A service of any kind, or the service in `--stack` | `--service seq` |
| `--db <NAME>` | package installation name | — | one target | A database | `--db orders-pg` |
| `--node <NODE>`, `-n` | node id, host name or display name | every node the service runs on | for `--ssh` and a raw port | Limit the grant to one node | `--node lab-c` |
| `--ssh` | flag | off | one target, with `--node` | The node's SSH port | `--node lab-b --ssh` |
| `--port <PORT>`, `-p` | integer 1–65535 | every port of the service | no | One port of the service; with `--node` and no service, a raw port | `--port 5341` |
| `--environment <ENVIRONMENT>` | environment name or id | — | no | The service's environment, when the name exists in more than one | `--environment web` |
| `--for <DURATION>` | `<n>m`, `<n>h`, `<n>d` or `never` | `8h` | no | How long the grant lasts | `--for 3d` |
| `--note <NOTE>` | string | — | no | Why the grant exists; shown in the list and the audit log | `--note "on-call review"` |

**Examples**

```sh
# Let a contractor reach the site app for an hour
forge tunnel grants create --user dev@example.com --app site --for 1h --note "on-call review"

# SSH to one node for two days
forge tunnel grants create --user ops@example.com --node lab-b --ssh --for 2d

# Only the ingest port of a stack service
forge tunnel grants create --user dev@example.com --stack seq-b --service seq --port 5341
```

**Output**

```text
$ forge tunnel grants create --user dev@example.com --app site --for 1h --note "on-call review"
dev@example.com may tunnel to site (web) on every node (every port) · expires in 1 h
{
  "created_at": 1794554593472,
  "created_by": "01900000-0000-7000-8000-000000000003",
  "expires_at": 1794558193472,
  "id": "01900000-0000-7000-8000-000000000041",
  "note": "on-call review",
  "revoked_at": null,
  "target": { "kind": "app", "ref": "site" },
  "user_id": "01900000-0000-7000-8000-000000000012"
}
```

Exit code `2` when no target or several are named, `4` without
`access.manage` or for `--for never` while the server does not allow it,
`6` for an unknown user, service or node.

### forge tunnel grants revoke

Revokes a grant. Tunnels that were opened through it close within seconds.
Needs `access.manage`; audited as `network.tunnel.grant.revoke`.

**Synopsis**

```text
forge tunnel grants revoke [OPTIONS] <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | grant id | — | yes | From `forge tunnel grants list` | `01900000-0000-7000-8000-000000000041` |
| `--yes` | flag | off | no | Do not ask first. Required with `--non-interactive` or without a terminal; the tunnels opened through the grant close | `--yes` |

**Examples**

```sh
# Revoke a grant without prompting
forge tunnel grants revoke <grant-id> --yes
```

**Output**

```text
$ forge tunnel grants revoke 01900000-0000-7000-8000-000000000041 --yes
{
  "revoked": "01900000-0000-7000-8000-000000000041"
}
```
