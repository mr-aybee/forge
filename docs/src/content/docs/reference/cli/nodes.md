---
title: forge nodes
description: Every forge nodes command for listing, tagging, renaming, enrolling, monitoring and opening shells on nodes, except the file commands.
sidebar:
  order: 3
---

`forge nodes` lists and manages the nodes (servers running the Forge agent)
you can see, and opens shells on them through Forge's SSH broker. For how
nodes join, see [Add nodes](/docs/guides/add-nodes/); for the file commands, see
[forge nodes files](/docs/reference/cli/nodes-files/). Every command takes the
[global flags](/docs/reference/cli/#global-flags).

Wherever a command takes `<node>`, you can pass the node's id, its host name
or its display name. Names match case-insensitively. A name that matches more
than one node exits with `2` (code `ambiguous`) and lists the matching ids;
pass an id instead. A name that matches no node you can see exits with `6`.

## Commands

| Command | What it does |
| --- | --- |
| [`forge nodes list`](#forge-nodes-list) | Lists the nodes you can see |
| [`forge nodes get`](#forge-nodes-get) | Shows one node |
| [`forge nodes tags`](#forge-nodes-tags) | Prints a node's tags, or changes them |
| [`forge nodes command`](#forge-nodes-command) | Sends a read-only command to a node as a job |
| [`forge nodes rename`](#forge-nodes-rename) | Sets the display name shown in the console |
| [`forge nodes dns-name`](#forge-nodes-dns-name) | Sets the node's local DNS name (`<label>.local`) |
| [`forge nodes files …`](/docs/reference/cli/nodes-files/) | Browses and manages files on a node (separate page) |
| [`forge nodes revoke`](#forge-nodes-revoke) | Revokes a node's agent certificate |
| [`forge nodes enroll-token`](#forge-nodes-enroll-token) | Creates a one-time enrollment token for a new node |
| [`forge nodes health`](#forge-nodes-health) | Shows a node's health observations, or one check's history |
| [`forge nodes metrics`](#forge-nodes-metrics) | Shows live metrics of one node, or the latest of every node |
| [`forge nodes history`](#forge-nodes-history) | Shows a node's resource history from the analytics database |
| [`forge nodes report`](#forge-nodes-report) | Shows a node report: trends, peaks and top processes |
| [`forge nodes analytics`](#forge-nodes-analytics) | Shows the analytics database's engine and state |
| [`forge nodes processes`](#forge-nodes-processes) | Shows the top processes on a node |
| [`forge nodes kill`](#forge-nodes-kill) | Ends a process, or a process and its descendants |
| [`forge nodes priority`](#forge-nodes-priority) | Changes a process's priority |
| [`forge nodes connectivity`](#forge-nodes-connectivity) | Shows or probes reachability between a node and its peers |
| [`forge nodes logins`](#forge-nodes-logins) | Lists the OS logins Forge may use on a node |
| [`forge nodes ssh`](#forge-nodes-ssh) | Shows a node's SSH endpoint and pinned host key |
| [`forge nodes terminal`](#forge-nodes-terminal) | Opens an interactive shell on a node |
| [`forge nodes exec`](#forge-nodes-exec) | Runs one command on a node without a terminal |

### forge nodes list

Lists the nodes you can see, with their status, OS, hardware, agent version
and cluster. Needs `view` on each node; nodes you cannot see are left out.

**Synopsis**

```text
forge nodes list [--cluster <CLUSTER>] [--workspace <WORKSPACE>] [--limit <N>] [--page <N>] [--all]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--cluster <CLUSTER>` | cluster id, name or abbreviation | the context's default cluster, else none | no | Only nodes in this cluster. Names match case-insensitively; a name that matches no cluster exits with `6`. Without the flag, the CLI uses the context's default cluster ([`forge contexts update --default-cluster`](/docs/reference/cli/contexts/#forge-contexts-update)) when one is set | `--cluster <cluster-id>` |
| `--workspace <WORKSPACE>` | workspace id | none | no | Only nodes in this workspace | `--workspace <workspace-id>` |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

**Examples**

```sh
# List every node you can see
forge nodes list

# List the nodes of one cluster
forge nodes list --cluster <cluster-name>

# Print the host names of offline nodes
forge -o json nodes list | jq -r '.result[] | select(.status != "online") | .hostname'
```

**Output**

```text
ID                                    HOSTNAME  NAME  STATUS  OS                                  SPECS                                AGENT                      CLUSTER
01900000-0000-7000-8000-000000000004  node-01   -     online  Linux (Debian GNU/Linux 13) x86_64  2 vCPU · 0.7 GiB RAM · 7.0 GiB disk  0.1.1-dev.20261113.214755  01900000-0000-7000-8000-000000000005
01900000-0000-7000-8000-000000000006  node-02   -     online  Linux (Debian GNU/Linux 13) x86_64  2 vCPU · 0.7 GiB RAM · 7.0 GiB disk  0.1.1-dev.20261113.214755  01900000-0000-7000-8000-000000000005
```

`SPECS` comes from the node's saved facts, or from live telemetry when there
are none. `-` means the value is unknown, for example on an offline node.

With `-o json`, each node also carries its facts, certificate and tags. The
CLI adds `os_label` and `specs`:

```json
{
  "result": [
    {
      "agent_version": "0.1.1-dev.20261113.214755",
      "arch": "x86_64",
      "cert_not_after": 1794640632919,
      "cert_serial": "0d3c0394c733f6524f39f9412649ffc1",
      "cluster_id": "01900000-0000-7000-8000-000000000005",
      "created_at": 1794554232919,
      "display_name": null,
      "facts": {
        "address": "192.0.2.11",
        "agent_version": "0.1.1-dev.20261113.214755",
        "arch": "x86_64",
        "collected_at_ms": 1794589874834,
        "cpus": 2,
        "disk_total_bytes": 7538470912,
        "hostname": "node-01",
        "kernel": "6.12.116+deb13-amd64",
        "mem_total_bytes": 707280896,
        "os": "linux",
        "os_version": "Linux (Debian GNU/Linux 13)",
        "ssh_principals": true
      },
      "hostname": "node-01",
      "id": "01900000-0000-7000-8000-000000000004",
      "last_seen_at": 1794590510975,
      "environment_id": null,
      "os": "linux",
      "os_label": "Linux (Debian GNU/Linux 13) x86_64",
      "role": "worker",
      "specs": "2 vCPU · 0.7 GiB RAM · 7.0 GiB disk",
      "status": "online",
      "tags": [],
      "workspace_id": "01900000-0000-7000-8000-000000000001"
    },
    …
  ],
  "schema": "forge.cli/v1"
}
```

Timestamps are epoch milliseconds.

Exit codes:

- `0`: success.
- `3`: not signed in.
- `9`: the server cannot be reached.

**See also**: [`forge nodes get`](#forge-nodes-get),
[`forge nodes metrics`](#forge-nodes-metrics),
[Monitor nodes](/docs/guides/node-monitoring/).

### forge nodes get

Shows one node. `forge nodes show` is an alias. Needs `view` on the node.

**Synopsis**

```text
forge nodes get <NODE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node to show | `web-01` |

**Examples**

```sh
# Show one node
forge nodes get <node>

# Print the node's IP address as Forge sees it
forge -o json nodes get <node> | jq -r '.result.facts.address'
```

**Output**

The default output is the node as a key/value list. Nested objects such as
`facts` are indented sections, empty fields are left out and epoch
milliseconds show as local date-times (`UTC` is added when the local offset
is unknown):

```text
agent_version   0.1.1-dev.20261113.214755
arch            x86_64
cert_not_after  2026-11-14 12:37:12
cert_serial     0d3c0394c733f6524f39f9412649ffc1
cluster_id      01900000-0000-7000-8000-000000000005
created_at      2026-11-13 12:37:12
facts:
  address          192.0.2.11
  …
  ssh_principals   yes
hostname        node-01
id              01900000-0000-7000-8000-000000000004
last_seen_at    2026-11-14 01:41:50
os              linux
role            worker
status          online
workspace_id    01900000-0000-7000-8000-000000000001
```

With `-o json`, the same object is wrapped:

```json
{
  "result": {
    "agent_version": "0.1.1-dev.20261113.214755",
    "hostname": "node-01",
    "id": "01900000-0000-7000-8000-000000000004",
    …
    "status": "online",
    "tags": [],
    "workspace_id": "01900000-0000-7000-8000-000000000001"
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: success.
- `2`: the name matches more than one node (`ambiguous`).
- `3`: not signed in.
- `4`: you lack `view` on the node.
- `6`: no node with this id or name that you can see.

**See also**: [`forge nodes list`](#forge-nodes-list),
[`forge nodes rename`](#forge-nodes-rename).

### forge nodes tags

Prints a node's tags, or changes them with `--set`, `--add` or `--remove`.
Reading needs `view` on the node. Changing needs `server.manage` on the node
(**Operator** or higher) and is audited as `tags.update`.

**Synopsis**

```text
forge nodes tags <NODE> [--set <SET>] [--add <ADD>] [--remove <REMOVE>]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node whose tags you read or change | `web-01` |
| `--set <SET>` | list | — | no | Replaces all tags. `--set ""` clears them. Cannot be combined with `--add` or `--remove` | `--set env:prod,web` |
| `--add <ADD>` | list | — | no | Adds these tags. Tags already present stay once | `--add web` |
| `--remove <REMOVE>` | list | — | no | Removes these tags. You can pass `--add` and `--remove` together | `--remove staging` |

Without a flag, the command prints the tags and changes nothing. The CLI
trims spaces and drops empty entries. The server then applies the tag rules:
a plain word or `key:value`, at most 64 characters per tag and 50 tags per
node, no commas or control characters, duplicates dropped
case-insensitively. See [Tags](/docs/guides/tags/) and the
[REST API](/docs/reference/rest-api/#tags).

:::caution
`--add` and `--remove` read the current tags, then write the whole new set.
A change someone else makes at the same moment can be overwritten.
:::

**Examples**

```sh
# Print a node's tags
forge nodes tags <node>

# Add two tags and remove one in a single change
forge nodes tags <node> --add env:prod,web --remove staging

# Replace every tag
forge nodes tags <node> --set dc:<site>,role:db

# Remove every tag
forge nodes tags <node> --set ""
```

**Output**

Reading, captured on a node without tags:

```text
{
  "tags": []
}
```

```json
{
  "result": {
    "tags": []
  },
  "schema": "forge.cli/v1"
}
```

A change prints the same shape with the tags as saved, after normalisation.

Exit codes:

- `0`: success.
- `2`: a tag breaks the rules, `--set` was combined with `--add` or
  `--remove`, or the name is ambiguous.
- `3`: not signed in.
- `4`: you lack `view` (read) or `server.manage` (change) on the node.
- `6`: no such node.

**See also**: [Tags](/docs/guides/tags/), [`forge nodes list`](#forge-nodes-list).

### forge nodes command

Sends a read-only command to the node as an `agent.command` job, waits for
it and prints the result. Needs `job.create` and `server.manage` on the node. Forge checks
both when you submit, and the job engine checks again before every step.

**Synopsis**

```text
forge nodes command <NODE> <KIND> [--name <NAME>] [--no-wait] [--timeout-secs <SECS>]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node that runs the command | `web-01` |
| `<kind>` | enum: `ping`, `collect_facts`, `service_status` | — | yes | `ping` checks that the agent answers. `collect_facts` refreshes the node's facts (OS, hardware, address, SSH host keys). `service_status` reports the state of one service | `collect_facts` |
| `--name <NAME>` | string | — | with `service_status` | The service to check. `service_status` without it exits with `2` | `--name nginx` |
| `--no-wait` | flag | off | no | Print the job id and return as soon as the job is queued (with `-o json`, the queued job) | `--no-wait` |
| `--timeout-secs <SECS>` | integer, seconds | `25` | no | How long to wait for the result. When it is not there in time the command exits with `8`; the job keeps running | `--timeout-secs 60` |

The command waits up to `--timeout-secs` for the job and prints the node's
answer as a key/value view (`-o json`: `{job_id, status, result}`). With
`--no-wait` it prints only the job id; follow the job with
[`forge jobs watch`](/docs/reference/cli/jobs/#forge-jobs-watch).

**Examples**

```sh
# Check that the agent answers
forge nodes command <node> ping

# Refresh the node's facts
forge nodes command <node> collect_facts

# Ask for the state of one service
forge nodes command <node> service_status --name nginx

# Queue a ping and keep only the job id
forge nodes command <node> ping --no-wait
```

**Output**

The queued job. Captured with `ping`; the default output is the same object
without the wrapper:

```json
{
  "result": {
    "attempts": 0,
    "context": {
      "mfa": true
    },
    "created_at": 1794590525506,
    "current_step": 0,
    "id": "01900000-0000-7000-8000-000000000008",
    "idempotency_key": null,
    "kind": "agent.command",
    "last_error": null,
    "lease_expires_at": null,
    "lease_owner": null,
    "lease_token": 0,
    "max_attempts": 3,
    "next_run_at": 1794590525506,
    "payload": {
      "command": {
        "type": "ping"
      },
      "server_id": "01900000-0000-7000-8000-000000000004"
    },
    "principal": {
      "id": "01900000-0000-7000-8000-000000000003",
      "type": "user"
    },
    "resource": {
      "id": "01900000-0000-7000-8000-000000000004",
      "kind": "server"
    },
    "state": {},
    "status": "queued",
    "updated_at": 1794590525506,
    "workspace_id": "01900000-0000-7000-8000-000000000001"
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: the job is queued. The job itself can still fail.
- `2`: unknown `<kind>`, `service_status` without `--name`, or an ambiguous
  name.
- `3`: not signed in.
- `4`: you lack `job.create` or `server.manage` on the node.
- `6`: no such node.

**See also**: [Agents](/docs/concepts/agents/), [Jobs](/docs/concepts/jobs/).

### forge nodes rename

Sets the display name shown in the console. The machine's host name does not
change. Needs `server.rename` on the node; audited as `server.rename`. A node that
has no [local name](/docs/concepts/local-names/) yet takes it from the display
name; an existing local name does not change (use
[`forge nodes dns-name`](#forge-nodes-dns-name)).

**Synopsis**

```text
forge nodes rename <NODE> <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node to rename | `web-01` |
| `<name>` | string | — | yes | The new display name: at most 63 characters, no control characters. An empty name, or the host name itself, removes the display name | `"Web 1 (Frankfurt)"` |

**Examples**

```sh
# Give a node a display name
forge nodes rename <node> "<display-name>"

# Remove the display name, so the console shows the host name again
forge nodes rename <node> ""
```

**Output**

The node after the change, in the same shape as
[`forge nodes get`](#forge-nodes-get), with `display_name` set. Not
captured on a live system.

Exit codes:

- `0`: success.
- `2`: the name is too long or has control characters, or `<node>` is
  ambiguous.
- `3`: not signed in.
- `4`: you lack `server.rename` on the node.
- `6`: no such node.

**See also**: [Monitor nodes](/docs/guides/node-monitoring/#rename-a-node),
[`forge nodes dns-name`](#forge-nodes-dns-name).

### forge nodes dns-name

Sets the node's DNS label, so every node resolves `<label>.local` (or
`<label>.<cluster>.local` in a cluster). Every node's hosts file follows
within 30 seconds. Needs `server.rename` on the node (**Operator** or
higher); audited as `server.dns_rename`. See [Local names](/docs/concepts/local-names/).

**Synopsis**

```text
forge nodes dns-name <NODE> <LABEL>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node to name | `lab-b` |
| `<label>` | DNS label | — | yes | 1–63 characters of `a-z`, `0-9` and `-`, not starting or ending with `-`; not `localhost`, `local` or another reserved name; unique in the workspace. `""` goes back to the label derived from the node's name | `db` |

**Examples**

```sh
# Name a node `db`: every node resolves db.local
forge nodes dns-name <node> db

# Go back to the derived label
forge nodes dns-name <node> ""
```

**Output**

```text
$ forge nodes dns-name lab-b db
{
  "label": "db",
  "label_set": true,
  "name": "db.lab.local",
  "server_id": "<server-id>"
}

$ forge nodes dns-name lab-c db
error: `db` is in use by node lab-b; rename that node first [conflict]
```

The second command exits with `7`. `label_set` is `false` after
`forge nodes dns-name <node> ""`, and `label` is then the derived label.


Exit codes:

- `0`: success.
- `2`: the label is not a valid DNS label, or `<node>` is ambiguous.
- `3`: not signed in.
- `4`: you lack `server.rename` on the node.
- `6`: no such node.
- `7`: another node has this label (`409`).

**See also**: [Rename a node and reach services by name](/docs/guides/local-names/),
[`forge dns list`](/docs/reference/cli/dns/).

### forge nodes revoke

Revokes the node's agent certificate, so the agent is refused on its next
request and must be enrolled again. Needs `server.manage` on the node
(**Operator** or higher); audited as `server.revoke`, including refused
attempts. The CLI asks you to type `yes` unless you pass `--yes`.

**Synopsis**

```text
forge nodes revoke [--code <CODE>] [--yes] <NODE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node to revoke | `web-01` |
| `--code <CODE>` | string (6 digits) | none | no | A one-time code from your authenticator app. Without it, the CLI asks for one on a terminal when the server requires MFA and you have enrolled an authenticator; otherwise it sends no code | `--code <one-time-code>` |
| `--yes` | flag | off | no | Skips the confirmation prompt. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

The server asks for two confirmations, and the CLI sends both after you
confirm: the node's host name, and the one-time code when one is needed.

:::caution
Revoking cuts the node off for good: to use it again, enroll it again with a
new token. In scripts that run where MFA is required, pass `--code`; without
a terminal the CLI cannot ask for it.
:::

**Examples**

```sh
# Revoke a node after a confirmation prompt (and a code prompt when MFA is required)
forge nodes revoke <node>

# Revoke a node without prompts, with a fresh one-time code
forge nodes revoke <node> --code <one-time-code> --yes
```

**Output**

The server answers `204 No Content`, so the default output prints nothing
and `-o json` prints a null result. Not captured on a live system.

```json
{
  "result": null,
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: the node is revoked.
- `1`: you did not type `yes` at the prompt (`cancelled`), or too many code
  attempts (`throttled`; try again in 15 minutes).
- `2`: no `--yes` in non-interactive use, a typed code that is not 6 digits,
  or an ambiguous name.
- `3`: not signed in.
- `4`: you lack `server.manage`, or the code is wrong or missing ("a valid
  authenticator code is required to revoke or remove a node").
- `5`: a code is needed and the CLI cannot ask for one (no terminal, or
  `--non-interactive`); pass `--code`.
- `6`: no such node.

**See also**: [Add nodes](/docs/guides/add-nodes/#revoke-a-node),
[`forge nodes remove`](#forge-nodes-remove),
[`forge nodes enroll-token`](#forge-nodes-enroll-token).

### forge nodes remove

Removes a node from Forge for good. The server revokes the node's agent
certificate, then deletes the node and everything Forge recorded for it:
services, stack parts, deploy slots, store memberships, file roots and
grants, tunnels, saved logins, SSH and Docker settings, metrics history,
alarms, its DNS name, tags and access assignments scoped to the node. Queued
jobs for the node are cancelled. The machine itself is not touched. Needs
`server.manage` on the node (**Operator** or higher); audited as
`server.remove`, including refused attempts. The CLI asks you to type `yes`
unless you pass `--yes`.

**Synopsis**

```text
forge nodes remove [--code <CODE>] [--yes] <NODE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node to remove | `web-01` |
| `--code <CODE>` | string (6 digits) | none | no | A one-time code from your authenticator app. Without it, the CLI asks for one on a terminal when the server requires MFA and you have enrolled an authenticator; otherwise it sends no code | `--code <one-time-code>` |
| `--yes` | flag | off | no | Skips the confirmation prompt. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

The server asks for the same two confirmations as
[`forge nodes revoke`](#forge-nodes-revoke): the node's host name, and the
one-time code when one is needed. You don't need to revoke the node first.

:::caution
Removing cannot be undone. A stack that used the node as its default node
moves to another of its nodes; a stack that ran only on this node is
deleted. Containers on the machine keep running until you stop them there.
The audit log keeps every record of the node.
:::

:::note
After removal, the node's host name and DNS name are free. Enroll the same
machine again with a new token and it joins as a fresh node with the same
name and no history.
:::

**Examples**

```sh
# Remove a node after a confirmation prompt (and a code prompt when MFA is required)
forge nodes remove <node>

# Remove a node without prompts, with a fresh one-time code
forge nodes remove <node> --code <one-time-code> --yes
```

**Output**

The server answers `204 No Content`. The CLI prints the removed node's id
and host name. Not captured on a live system.

```json
{
  "result": {
    "hostname": "web-01",
    "removed": "<node-id>"
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: the node is removed.
- `1`: you did not type `yes` at the prompt (`cancelled`), or too many code
  attempts (`throttled`; try again in 15 minutes).
- `2`: no `--yes` in non-interactive use, a typed code that is not 6 digits,
  or an ambiguous name.
- `3`: not signed in.
- `4`: you lack `server.manage`, or the code is wrong or missing ("a valid
  authenticator code is required to revoke or remove a node").
- `5`: a code is needed and the CLI cannot ask for one (no terminal, or
  `--non-interactive`); pass `--code`.
- `6`: no such node.
- `7`: the node is the control node ("the control node cannot be removed").

**See also**: [Add nodes](/docs/guides/add-nodes/#remove-a-node),
[`forge nodes revoke`](#forge-nodes-revoke).

### forge nodes enroll-token

Creates a one-time enrollment token for a new node and prints it once.
Needs `server.enroll` on the workspace, and on the cluster when you pass
`--cluster` (**Operator** or higher). Audited as
`server.enrollment_token.create`; the audit entry holds the token id, never
the token.

**Synopsis**

```text
forge nodes enroll-token [--cluster <CLUSTER>] [--hostname <HOSTNAME>] [--ttl-minutes <TTL_MINUTES>]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--cluster <CLUSTER>` | cluster id or name | none | no | The cluster the node joins when it enrolls | `--cluster prod` |
| `--hostname <HOSTNAME>` | string | none | no | The host name you expect. The onboarding list shows it until the node enrolls. Trimmed; ignored when empty or longer than 253 characters | `--hostname web-03` |
| `--ttl-minutes <TTL_MINUTES>` | integer (minutes) | `60` | no | How long the token is valid. The server clamps it to 1 to 1440 (24 hours) | `--ttl-minutes 30` |

:::caution
The token enrolls one node into your workspace. Treat it as a secret: do not
paste it into tickets or chat, and do not log the command's output. The
install commands in the output prompt for the token, so it stays out of the
node's command line and shell history.
:::

**Examples**

```sh
# Create a token valid for 60 minutes
forge nodes enroll-token

# Create a token for a node that joins a cluster, valid for 30 minutes
forge nodes enroll-token --cluster <cluster> --hostname <node-name> --ttl-minutes 30
```

**Output**

In table mode the CLI first prints `This token is shown once; it enrolls one
node.` on stderr, then the object below without the wrapper. Shape from the
server's schema; not captured on a live system.

```json
{
  "result": {
    "id": "<token-id>",
    "token": "<enrollment-token>",
    "expires_at": 1794594125506,
    "install": {
      "linux": "printf 'Enrollment token: '; stty -echo; read -r FORGE_ENROLL_TOKEN; stty echo; echo; export FORGE_ENROLL_TOKEN; forge-agent enroll --server <agent-url>; unset FORGE_ENROLL_TOKEN",
      "windows": "$t = Read-Host 'Enrollment token' -AsSecureString; … .\\forge-agent.exe enroll --server <agent-url>; Remove-Item Env:FORGE_ENROLL_TOKEN"
    }
  },
  "schema": "forge.cli/v1"
}
```

`expires_at` is epoch milliseconds. `<agent-url>` is the server's
`agent_public_url` (see [Configuration](/docs/reference/configuration/)).

Exit codes:

- `0`: the token was created.
- `2`: the cluster belongs to another workspace.
- `3`: not signed in.
- `4`: you lack `server.enroll` on the workspace or the cluster.
- `6`: no such cluster.

**See also**: [Add nodes](/docs/guides/add-nodes/),
[forge-agent](/docs/reference/cli/forge-agent/),
[Node installer](/docs/reference/cli/node-installer/).

### forge nodes health

Shows the health observations of a node: the current state of every check,
or the stored history of one check. Needs `health.view` on the node.

**Synopsis**

```text
forge nodes health <NODE> [--prefix <PREFIX>] [--history <SUBJECT>] [--limit <N>]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | none | yes | The node whose observations you want | `web-01` |
| `--prefix` | subject prefix | every subject | no | Only subjects starting with this | `health.readiness` |
| `--history` | subject | none | no | Show this subject's history instead of current values | `health.liveness.web` |
| `--limit` | integer, 1-1000 | `200` | no | Most rows to show | `50` |

Subjects are lowercase dotted names (`a-z`, `0-9`, `.`, `-`, `_`). The CLI
refuses anything else before it calls the server.

`STATE` is the value to act on now. When no fresh observation arrived in
time, it reads `unknown` and `STALE` says `yes`; `LAST KNOWN` keeps the last
reported state. Freshness is measured on the server's clock. See
[Health observations](/docs/concepts/health-observations/).

**Examples**

```sh
# Current state of every check on a node
forge nodes health <node>

# Only readiness checks
forge nodes health <node> --prefix health.readiness

# The last 20 results of one check
forge nodes health <node> --history health.liveness.web --limit 20

# Subjects that are failing right now
forge -o json nodes health <node> | jq -r '.result.observations[] | select(.effective.state == "failing") | .key.subject'
```

**Output**

Current values, as a table:

```text
SUBJECT                  STATE    LAST KNOWN  REASON   STALE
health.liveness.web      passing  passing              no
health.readiness.web     unknown  failing     timeout  yes
```

With `--history`, newest first. `RECEIVED (MS)` is epoch milliseconds on the
server clock:

```text
RECEIVED (MS)  STATE    REASON   DETAIL
1763120420000  failing  timeout  connect timed out after 2s
1763120300000  passing
```

With `-o json`, the API response. Each observation has `key`
(`resource`, `server`, `subject`), `value`, `effective`, `stale`, `reason`,
`detail`, `source`, `executor`, `generation`, `observed_at`, `received_at`,
`fresh_until`, `skewed` and `changed_at`. See
[REST API: Observations](/docs/reference/rest-api/#observations).

**Errors**

| Error | Cause | Fix |
| --- | --- | --- |
| `prefix ... must use lowercase letters, digits, ...` | The prefix or subject has other characters | Use the subject as the node reports it |
| `403 Forbidden` | You lack `health.view` on the node | Ask for a role on the node's environment or workspace |

### forge nodes history

Shows a node's CPU, memory, disk, network and load history from the
analytics database. Windows up to 48 hours return one point per minute;
longer windows return one point per hour. Needs `view` on the node.

**Synopsis**

```text
forge nodes history <NODE> [--hours <HOURS>]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<NODE>` | node id, host name or display name | none | yes | The node | `web-01` |
| `--hours` | integer, 1–9600 | `24` | no | Hours back from now | `--hours 168` |

**Examples**

```sh
# The last 24 hours, one point per minute
forge nodes history <node>

# The highest CPU minute of the last two days
forge -o json nodes history <node> --hours 48 | jq '.result.points | max_by(.cpu_max)'
```

**Output**

```json
{
  "result": {
    "server_id": "0193f5c2-…",
    "from": 1763460000000,
    "to": 1763546400000,
    "resolution": "minute",
    "points": [
      { "t": 1763460000000, "cpu_avg": 12.4, "cpu_max": 31.0, "mem_used": 2147483648, "mem_total": 8589934592,
        "disk_used": 21474836480, "disk_total": 107374182400, "rx": 5120, "tx": 2048, "load1": 0.42 }
    ]
  }
}
```

`t` is the start of the minute or hour in epoch milliseconds. `cpu_avg` and
`cpu_max` are percentages; `rx` and `tx` are bytes per second.

**Errors**

| Message | Cause | Fix |
| --- | --- | --- |
| `analytics is off on this server` | `[analytics] engine = "off"` | Choose an engine; see [Choose and connect databases](/docs/guides/databases/) |
| `analytics is unavailable: …` | The analytics database cannot be reached | Check `forge nodes analytics` and the engine |

### forge nodes report

Shows a node report for a window: the same history as
[`forge nodes history`](#forge-nodes-history), the peak CPU, memory and disk
with when they happened, and the ten processes that used the most CPU and the
most memory. Needs `view` on the node.

**Synopsis**

```text
forge nodes report <NODE> [--hours <HOURS>]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<NODE>` | node id, host name or display name | none | yes | The node | `web-01` |
| `--hours` | integer, 1–9600 | `168` | no | Hours back from now | `--hours 24` |

**Examples**

```sh
# The last 7 days
forge nodes report <node>

# The process that used the most CPU this week
forge -o json nodes report <node> | jq '.result.top_cpu[0]'
```

**Output**

```json
{
  "result": {
    "resolution": "hour",
    "points": [ … ],
    "cpu_peak": { "t": 1763510400000, "value": 97.5 },
    "mem_peak": { "t": 1763514000000, "value": 81.2 },
    "disk_peak": { "t": 1763546400000, "value": 43.0 },
    "top_cpu": [
      { "name": "postgres", "user": "postgres", "cpu_avg": 22.1, "cpu_max": 88.0,
        "mem_avg": 524288000, "mem_max": 786432000, "minutes": 9840, "cmd": "postgres: checkpointer" }
    ],
    "top_mem": [ … ]
  }
}
```

Peak `value` is a percentage. `minutes` counts the minutes the process was in
the node's top list.

### forge nodes analytics

Shows the analytics database: the engine, whether it is ready, the last
error, rows waiting to be written and rows dropped because the buffer was
full. Needs `view` on the platform. It takes no arguments.

**Synopsis**

```text
forge nodes analytics
```

**Examples**

```sh
# Analytics database status
forge nodes analytics
# Rows dropped because the buffer was full
forge -o json nodes analytics | jq '.result.dropped_rows'
```

**Output**

```json
{
  "result": {
    "engine": "embedded",
    "ready": true,
    "last_error": null,
    "pending_rows": 0,
    "dropped_rows": 0,
    "retention": { "node_metrics_days": 7, "top_processes_days": 7, "node_rollup_days": 365, "app_logs_days": 30 }
  }
}
```

### forge nodes metrics

Without a node, shows the latest CPU, load, memory and disk of every node you
can see. With a node, shows its recent metric samples. Needs `view` on the
node.

**Synopsis**

```text
forge nodes metrics [NODE]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | every node | no | The node whose sample history you want | `web-01` |

Asking for one node marks it as watched: its agent then reports every couple
of seconds (`live_interval_secs`) for a while. The history is kept in the
server's memory only.

**Examples**

```sh
# Latest metrics of every node
forge nodes metrics

# Recent samples of one node
forge nodes metrics <node>

# The newest CPU value of one node
forge -o json nodes metrics <node> | jq '.result.samples[-1].cpu'
```

**Output**

Every node, as a table. `CPU`, `MEM` and `DISK` are percentages; `LOAD` is
the one-minute load average:

```text
NODE     CPU  LOAD                 MEM  DISK
node-01  0%   0.10999999940395357  54%  31%
node-02  0%   0.07000000029802322  63%  33%
```

With `-o json`, the raw latest sample per node id. Byte counts are bytes,
`t` is epoch milliseconds, `uptime` is seconds, `rx` and `tx` are network
bytes per second:

```json
{
  "result": {
    "01900000-0000-7000-8000-000000000004": {
      "cpu": 0.4962778985500336,
      "cpus": 2,
      "disk_total": 7538470912,
      "disk_used": 2335244288,
      "load1": 0.10999999940395357,
      "mem_total": 707280896,
      "mem_used": 382472192,
      "rx": 95,
      "swap_total": 838856704,
      "swap_used": 20037632,
      "t": 1794590510975,
      "tx": 217,
      "uptime": 36819
    },
    …
  },
  "schema": "forge.cli/v1"
}
```

One node, in both formats an object with `online`, `live_interval_secs`,
`samples` (oldest first), `server_id` and `volumes` (mounted volumes from the
latest sample). Trimmed:

```json
{
  "result": {
    "live_interval_secs": 2,
    "online": true,
    "samples": [
      {
        "cpu": 0.7633588,
        "cpus": 2,
        "disk_total": 7538470912,
        "disk_used": 2335219712,
        "load1": 0.0,
        "mem_total": 707280896,
        "mem_used": 393945088,
        "rx": 153,
        "swap_total": 838856704,
        "swap_used": 20037632,
        "t": 1794589863750,
        "tx": 1214,
        "uptime": 36172
      },
      …
    ],
    …
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: success.
- `2`: the name is ambiguous.
- `3`: not signed in.
- `4`: you lack `view` on the node.
- `6`: no such node.

**See also**: [Monitor nodes](/docs/guides/node-monitoring/#watch-live-metrics),
[`forge nodes processes`](#forge-nodes-processes).

### forge nodes processes

Shows the top processes on the node, read live from its agent. Needs
`server.manage` on the node. The node must be online.

**Synopsis**

```text
forge nodes processes <NODE> [--sort <SORT>] [--limit <LIMIT>] [--filter <FILTER>]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node to inspect | `web-01` |
| `--sort <SORT>` | enum: `cpu`, `mem`, `pid`, `time`, `name` | `cpu` | no | The sort order | `--sort mem` |
| `--limit <LIMIT>` | integer | `25` | no | How many processes to show. The server clamps it to 1 to 500 | `--limit 10` |
| `--filter <FILTER>` | string | none | no | Only processes whose command line contains this text. The server uses the first 100 characters | `--filter nginx` |

**Examples**

```sh
# The 25 busiest processes
forge nodes processes <node>

# The 10 processes that use the most memory
forge nodes processes <node> --sort mem --limit 10

# Only nginx processes, as JSON
forge -o json nodes processes <node> --filter nginx
```

**Output**

Captured with `--limit 5`:

```text
PID    USER         CPU                  MEM%  COMMAND
18153  forge-agent  0.20000000298023224  1.4   forge-agent
3709   root         0.20000000298023224  4.2   containerd
1      root         0.10000000149011612  1.5   systemd
18152  root         0.10000000149011612  1.1   forge-agent-hel
262    root         0.0                  2.4   systemd-journal
```

With `-o json`, every field the agent reports for each process. `mem` and
`virt` are bytes, `started` is epoch seconds, `time` is the seconds since
the process started.
Captured on Linux: the first row of `--filter "sleep 361" --sort pid` (formatted; the CLI prints
one line):

```json
{
  "result": [
    {
      "cmd": "sleep 3611",
      "cpu": 0.0,
      "mem": 2080768,
      "mem_pct": 0.3,
      "name": "sleep",
      "nice": 0,
      "pid": 22922,
      "ppid": 1,
      "started": 1794605137,
      "state": "S",
      "time": 4,
      "user": "<user>",
      "virt": 5705728
    }
  ],
  "schema": "forge.cli/v1"
}
```

The fields differ by the node's OS:

| Field | Linux | Windows |
| --- | --- | --- |
| `user` | The owner's name from `/etc/passwd`, else the uid | The owner's account: `NT AUTHORITY\SYSTEM`, `NT AUTHORITY\LOCAL SERVICE`, `NT AUTHORITY\NETWORK SERVICE`, `Window Manager\DWM-<n>`, `Font Driver Host\UMFD-<n>` or a local account name. Other owners, such as domain accounts, show as the SID (`S-1-5-21-…`) |
| `state` | The htop letter: `R`, `S`, `D`, `Z`, `T`, `X` or `?` | `null` |
| `nice` | Integer, `-20` to `19` | Not present. Forge does not read a process's priority class on Windows |

`started` is the value that [`forge nodes kill`](#forge-nodes-kill) and
[`forge nodes priority`](#forge-nodes-priority) pass as `--start-time`.

The CLI prints the process list only. The server's answer also carries
`summary` (with `os` and `load`, which is `null` on Windows) and `actions`
(what the node can do to a process). To see them, call the endpoint with
[`forge api request`](/docs/reference/cli/api/#forge-api-request)
`GET /api/v1/servers/<node-id>/processes`. See the
[REST API reference](/docs/reference/rest-api/#processes) for every field.

Exit codes:

- `0`: success.
- `2`: an unknown `--sort` value, or an ambiguous name.
- `3`: not signed in.
- `4`: you lack `server.manage` on the node.
- `6`: no such node.
- `7`: the node is offline, or its agent refused the request (older agents
  do not support it; update them).
- `8`: the node did not answer within 20 seconds.

**See also**: [Monitor nodes](/docs/guides/node-monitoring/#inspect-processes),
[`forge nodes kill`](#forge-nodes-kill),
[`forge nodes metrics`](#forge-nodes-metrics).

### forge nodes kill

Ends a process on the node, or with `--tree` the process and its
descendants. On Linux it sends a signal; on Windows it ends the task (`term`)
or the process (`kill`). Needs `process.manage` on the node (**Operator** or
higher) and an MFA-verified session. Audited as `server.process.signal`,
also when the node refuses. The node must be online. The CLI asks you to
type `yes` unless you pass `--yes`.

**Synopsis**

```text
forge nodes kill <NODE> <PID> [--signal <SIGNAL>] [--tree] [--start-time <START_TIME>] [--yes]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node the process runs on | `web-01` |
| `<pid>` | integer | — | yes | The process id, as [`forge nodes processes`](#forge-nodes-processes) shows it. `0` is refused | `4242` |
| `--signal <SIGNAL>` | enum: Linux `term`, `kill`, `hup`, `int`; Windows `term`, `kill` | `term` | no | Linux: `term` sends `SIGTERM`, `kill` sends `SIGKILL`, `hup` sends `SIGHUP`, `int` sends `SIGINT`. Windows: `term` is **End task** (`taskkill` without `/F`, asks the process to close), `kill` is **End process** (`taskkill /F`, forced). Another value exits with `2` | `--signal kill` |
| `--tree` | flag | off | no | Acts on the process's descendants too: the process first, then its children and their children | `--tree` |
| `--start-time <START_TIME>` | integer (seconds since the epoch) | read from the node's process list | no | The process's `started` value. Without it, the CLI reads the node's first 500 processes sorted by PID and takes the value from there. A PID that is not in that list exits with `6` | `--start-time 1794589874` |
| `--yes` | flag | off | no | Skips the confirmation prompt. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

The node acts only when the PID still belongs to the process that started
at `--start-time`. When the process exited and its PID was reused, the node
refuses. On Linux the node pins the process before it checks, so a reused
PID never gets the signal.

The node refuses protected processes, and checks the whole tree before it
acts on any of them:

| OS | Protected |
| --- | --- |
| Both | PID `0`; every Orbyn Forge component (process names `forge` and `forge-*`, such as `forge-agent`, its helper and a co-located `forge-server`) |
| Linux | PID `1` (init), PID `2` and kernel threads (children of PID `2`) |
| Windows | PID `4`, `System`, `Registry`, `Secure System`, `Memory Compression`, `smss`, `csrss`, `wininit`, `services`, `lsass`, `LsaIso`, `winlogon` |

:::caution
`kill` (`SIGKILL`, or **End process** on Windows) gives the process no
chance to save its state or clean up. Prefer `term` first. On Windows, the
agent runs as a service in session 0, so `term` usually fails for desktop
apps and console programs with "This process can only be terminated
forcefully". Use `kill` for those.
:::

**Examples**

```sh
# Ask a process to exit (SIGTERM on Linux, End task on Windows)
forge nodes kill <node> <pid>

# Force a process and all its descendants to end, without a prompt
forge nodes kill <node> <pid> --signal kill --tree --yes

# Ask a Linux daemon to reload its configuration
forge nodes kill <node> <pid> --signal hup

# Act only on the process that started at a known time
forge nodes kill <node> <pid> --start-time <started> --yes
```

**Output**

The node's answer. `action` echoes what the node ran, `affected` lists the
PIDs it acted on (the process first, then its descendants), and `exited`
lists descendants that had exited before Forge reached them. Captured on
Linux with `forge nodes kill <node> 23021 --tree --yes`, where `23021` is a
shell waiting for two `sleep` processes:

```json
{
  "action": {
    "kind": "signal",
    "pid": 23021,
    "signal": "term",
    "start_time": 1794605166,
    "tree": true
  },
  "affected": [
    23021,
    23024,
    23023
  ],
  "exited": [],
  "pid": 23021
}
```

With `-o json`, the same object inside the wrapper (formatted; the CLI
prints one line):

```json
{
  "result": {
    "action": {
      "kind": "signal",
      "pid": 22922,
      "signal": "term",
      "start_time": 1794605137,
      "tree": false
    },
    "affected": [22922],
    "exited": [],
    "pid": 22922
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: the node acted on the process.
- `1`: you did not type `yes` at the prompt (`cancelled`).
- `2`: no `--yes` in non-interactive use, a signal the node's OS does not
  have, PID `0`, or an ambiguous name.
- `3`: not signed in.
- `4`: you lack `process.manage` on the node.
- `5`: an MFA-verified session is needed and the CLI cannot prompt (run
  [`forge auth step-up`](/docs/reference/cli/auth/#forge-auth-step-up) first).
- `6`: no such node, or the PID is not in the node's list ("process
  `<pid>` is not in the node's list; check the PID or pass --start-time").
- `7`: the node is offline, or it refused: "the node refused: `<reason>`".
  The reason says the process is protected, is no longer the one listed,
  has exited, can only be ended forcefully, or that the agent does not
  allow `process_action`.
- `8`: the node did not answer within 30 seconds.

**See also**: [Monitor nodes](/docs/guides/node-monitoring/#end-a-process-or-change-its-priority),
[`forge nodes priority`](#forge-nodes-priority),
[`forge nodes processes`](#forge-nodes-processes).

### forge nodes priority

Changes the priority of a process on the node: the nice value on Linux, the
priority class on Windows. Needs `process.manage` on the node (**Operator**
or higher) and an MFA-verified session. Audited as `server.process.priority`,
also when the node refuses. The node must be online. The CLI does not ask
for confirmation.

**Synopsis**

```text
forge nodes priority <NODE> <PID> [--nice <NICE>] [--class <CLASS>] [--start-time <START_TIME>]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node the process runs on | `web-01` |
| `<pid>` | integer | — | yes | The process id, as [`forge nodes processes`](#forge-nodes-processes) shows it. `0` is refused | `4242` |
| `--nice <NICE>` | integer, `-20` to `19` | none | on Linux | The new nice value. `-20` is the highest priority, `19` the lowest. Negative values work without `=`. Conflicts with `--class` | `--nice -5` |
| `--class <CLASS>` | enum: `idle`, `below_normal`, `normal`, `above_normal`, `high` | none | on Windows | The new priority class. Forge never sets `realtime`. Conflicts with `--nice` | `--class below_normal` |
| `--start-time <START_TIME>` | integer (seconds since the epoch) | read from the node's process list | no | The process's `started` value. Without it, the CLI reads the node's first 500 processes sorted by PID and takes the value from there. A PID that is not in that list exits with `6` | `--start-time 1794589874` |

Pass exactly one of `--nice` and `--class`, the one that matches the node's
OS. Neither exits with `2`. The same start-time check and the same protected
processes apply as for [`forge nodes kill`](#forge-nodes-kill). On Windows,
the node changes the class through the built-in Windows PowerShell, which
takes about half a second.

**Examples**

```sh
# Lower the priority of a busy Linux process
forge nodes priority <node> <pid> --nice 10

# Raise the priority of a Linux process
forge nodes priority <node> <pid> --nice -5

# Lower the priority class of a Windows process
forge nodes priority <node> <pid> --class below_normal
```

**Output**

The node's answer. With `-o json` it comes inside the
`{"result", "schema"}` wrapper. Captured on Linux with
`forge nodes priority <node> 23020 --nice 10`:

```json
{
  "action": {
    "kind": "nice",
    "nice": 10,
    "pid": 23020,
    "start_time": 1794605166
  },
  "affected": [
    23020
  ],
  "exited": [],
  "pid": 23020
}
```

Windows, with `--class below_normal` (the node's answer for a test `ping`;
Windows then reports the process as `BelowNormal`):

```json
{
  "action": {
    "class": "below_normal",
    "kind": "priority_class",
    "pid": 18644,
    "start_time": 1794607457
  },
  "affected": [
    18644
  ],
  "exited": [],
  "pid": 18644
}
```

Exit codes:

- `0`: the node changed the priority.
- `2`: neither `--nice` nor `--class`, both, the wrong one for the node's
  OS, a value out of range, PID `0`, or an ambiguous name.
- `3`: not signed in.
- `4`: you lack `process.manage` on the node.
- `5`: an MFA-verified session is needed and the CLI cannot prompt (run
  [`forge auth step-up`](/docs/reference/cli/auth/#forge-auth-step-up) first).
- `6`: no such node, or the PID is not in the node's list.
- `7`: the node is offline, or it refused: "the node refused: `<reason>`".
- `8`: the node did not answer within 30 seconds.

**See also**: [Monitor nodes](/docs/guides/node-monitoring/#end-a-process-or-change-its-priority),
[`forge nodes kill`](#forge-nodes-kill),
[`forge nodes processes`](#forge-nodes-processes).

### forge nodes connectivity

Shows the node's peers and the last reachability checks from this node to
them, or runs a new check with `--probe`. Reading needs `view` on the node.
`--probe` needs `server.manage` on the node and runs as an `agent.command`
job, which the job engine authorizes and audits.

**Synopsis**

```text
forge nodes connectivity <NODE> [--probe]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node the checks start from | `web-01` |
| `--probe` | flag | off | no | Probes every peer now (four echo requests each) instead of showing the last result. The node must be online and have peers in its cluster. Waits up to 40 seconds | `--probe` |

**Examples**

```sh
# Show the peers and the last check results
forge nodes connectivity <node>

# Probe every peer now
forge nodes connectivity <node> --probe
```

**Output**

Without `--probe`, captured on a node that has not been probed yet (`runs`
is empty):

```text
{
  "peers": [
    {
      "hostname": "node-02",
      "role": "worker",
      "server_id": "01900000-0000-7000-8000-000000000006",
      "status": "online",
      "target": "192.0.2.12:22"
    }
  ],
  "runs": []
}
```

```json
{
  "result": {
    "peers": [
      {
        "hostname": "node-02",
        "role": "worker",
        "server_id": "01900000-0000-7000-8000-000000000006",
        "status": "online",
        "target": "192.0.2.12:22"
      }
    ],
    "runs": []
  },
  "schema": "forge.cli/v1"
}
```

With `--probe`, the new run. Each result carries the peer's
`server_id`, `hostname` and `role` next to the agent's measurements. Shape
from the server's schema; not captured on a live system.

```json
{
  "result": {
    "status": "succeeded",
    "job_id": "<job-id>",
    "run": {
      "t": 1794590525506,
      "results": [
        { "server_id": "<peer-id>", "hostname": "<peer-name>", "role": "worker", … }
      ]
    }
  },
  "schema": "forge.cli/v1"
}
```

When the probe job does not succeed, the CLI prints the job's result as the
server returned it.

Exit codes:

- `0`: success.
- `2`: with `--probe`, the node is offline or has no peers (add it to a
  cluster first); or the name is ambiguous.
- `3`: not signed in.
- `4`: you lack `view`, or `server.manage` for `--probe`.
- `6`: no such node.

**See also**:
[Monitor nodes](/docs/guides/node-monitoring/#check-connectivity-between-nodes),
[Clusters](/docs/concepts/clusters/).

### forge nodes logins

Lists the OS logins saved for the node. The console offers them when you
open a terminal, and `terminal` and `exec` use the first one by default.
Needs `view` on the node.

**Synopsis**

```text
forge nodes logins <NODE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node whose saved logins you want | `web-01` |

:::note
The list is a convenience, not a grant. Opening a session still needs
`terminal.open` on the node, and the account must exist on the node. To
change the list, see [Browser terminal (SSH)](/docs/guides/terminal/#open-a-terminal).
:::

**Examples**

```sh
# List the saved logins
forge nodes logins <node>
```

**Output**

Captured on a node with no saved logins:

```text
{
  "logins": []
}
```

```json
{
  "result": {
    "logins": []
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: success.
- `2`: the name is ambiguous.
- `3`: not signed in.
- `4`: you lack `view` on the node.
- `6`: no such node.

**See also**: [`forge nodes terminal`](#forge-nodes-terminal),
[`forge nodes exec`](#forge-nodes-exec).

### forge nodes ssh

Shows the node's SSH endpoint (host, port, pinned host-key fingerprint) and
the public key of Forge's SSH user CA, which the node must trust. Needs
`view` on the node.

**Synopsis**

```text
forge nodes ssh <NODE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node whose SSH settings you want | `web-01` |

**Examples**

```sh
# Show the SSH endpoint and the CA key
forge nodes ssh <node>

# Print only the CA public key, for the node's TrustedUserCAKeys file
forge -o json nodes ssh <node> | jq -r '.result.ca_public_key'
```

**Output**

```text
{
  "ca_public_key": "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEXAMPLEexampleEXAMPLEexampleEXAMPLEexampleEXAM orbyn-forge-user-ca",
  "endpoint": {
    "host": "192.0.2.11",
    "host_key_fingerprint": "SHA256:<host-key-fingerprint>",
    "port": 22,
    "server_id": "01900000-0000-7000-8000-000000000004",
    "updated_at": 1794554235752
  }
}
```

```json
{
  "result": {
    "ca_public_key": "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEXAMPLEexampleEXAMPLEexampleEXAMPLEexampleEXAM orbyn-forge-user-ca",
    "endpoint": {
      "host": "192.0.2.11",
      "host_key_fingerprint": "SHA256:<host-key-fingerprint>",
      "port": 22,
      "server_id": "01900000-0000-7000-8000-000000000004",
      "updated_at": 1794554235752
    }
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: success.
- `2`: the name is ambiguous.
- `3`: not signed in.
- `4`: you lack `view` on the node.
- `6`: no such node.

**See also**:
[Set up SSH access for a node](/docs/guides/terminal/#set-up-ssh-access-for-a-node).

### forge nodes terminal

Opens an interactive shell on the node through Forge's SSH broker. Needs
`terminal.open` on the node (**Operator** or higher) and an MFA-verified
session when the server requires MFA. Audited as `terminal.open` and
`terminal.close`.

**Synopsis**

```text
forge nodes terminal [-l <LOGIN>] <NODE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node to open a shell on | `web-01` |
| `-l`, `--login <LOGIN>` | string | the node's first saved login, else `root` | no | The OS account to log in as. 1 to 32 letters, digits, `_`, `-` or `.`. The account must exist on the node | `--login deploy` |

How it works:

- It needs a terminal on both ends: stdin and stdout must be a TTY. In
  scripts or with piped input, the command exits with `2`; use
  [`forge nodes exec`](#forge-nodes-exec) there. It works from Linux, macOS
  and Windows terminals, and resizing the window resizes the remote terminal.
- The CLI first runs the server's checks as a plain request
  (`/terminal/check`). A missing permission or SSH endpoint is reported
  plainly, and when the server wants MFA, the CLI asks for a one-time code
  before the shell opens.
- The CLI sends its bearer token in a header over the WebSocket, never in the
  URL. The server signs a certificate for the login that is valid for five
  minutes; the CLI never sees an SSH key.
- The session ends when the shell exits. The server also ends it after
  30 minutes without input, after 12 hours, within a minute of your sign-in
  session ending or `terminal.open` being withdrawn, or when someone revokes
  it with [`forge sessions revoke`](/docs/reference/cli/sessions/#forge-sessions-revoke).
- On Windows nodes, OpenSSH Server must be installed and trust Forge's CA;
  pass `--login`, because `root` does not exist there.

**Examples**

```sh
# Open a shell as the node's default login
forge nodes terminal <node>

# Open a shell as a given account
forge nodes terminal <node> --login <login>
```

**Output**

The remote terminal, in your terminal. Before it, the CLI prints
`Connecting to <login>@<host>… (the session ends when the shell exits)` on
stderr. When the session ends it prints `Connection closed.`, or
`Connection closed (<reason>).` for reasons such as `idle_timeout`,
`max_lifetime` or `session_ended`. The command prints no JSON. Not captured,
because it needs an interactive terminal.

Exit codes:

- `0`: the session closed (the shell exited, or a time limit ended it).
- `1`: the session was revoked (code `revoked`), or the server could not
  connect to the node.
- `2`: no terminal on stdin or stdout, an invalid login name, or an
  ambiguous name.
- `3`: not signed in.
- `4`: you lack `terminal.open` on the node.
- `5`: an MFA-verified session is needed and the CLI cannot prompt for a
  code (run [`forge auth step-up`](/docs/reference/cli/auth/#forge-auth-step-up) first).
- `6`: no such node.
- `7`: the node has no SSH endpoint and did not report a host key (is
  OpenSSH installed?).
- `9`: the server cannot be reached, or the connection broke.

**See also**: [Browser terminal (SSH)](/docs/guides/terminal/),
[`forge sessions`](/docs/reference/cli/sessions/),
[`forge nodes exec`](#forge-nodes-exec).

### forge nodes exec

Runs one command on the node without a terminal, with stdout and stderr
kept apart. Needs `terminal.open` on the node (**Operator** or higher) and
an MFA-verified session when the server requires MFA. Audited as
`terminal.exec` and `terminal.exec_close`.

**Synopsis**

```text
forge nodes exec [-l <LOGIN>] [--shell] [--propagate-exit-code] <NODE> -- <ARGV>...
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node that runs the command | `web-01` |
| `<argv>` | words after `--` | — | yes | The command and its arguments. Each word is quoted for the remote POSIX shell on its own, so it reaches the command unchanged | `-- systemctl status nginx` |
| `-l`, `--login <LOGIN>` | string | the node's first saved login, else `root` | no | The OS account that runs the command | `--login deploy` |
| `--shell` | flag | off | no | Joins the words with spaces and sends them as one shell command line, so pipes, `&&`, redirections and `$VARS` work | `--shell -- 'df -h \| grep /var'` |
| `--propagate-exit-code` | flag | off | no | Exits with the remote command's status. Without it, `forge` exits `0` once the command ran and prints a non-zero status on stderr | `--propagate-exit-code` |

How it works:

- Piped stdin is forwarded to the command, then end-of-file. When stdin is a
  terminal, the command gets no input.
- Output streams as it arrives: remote stdout to your stdout, remote stderr
  to your stderr.
- With `-o json`, the CLI collects the output and prints one object at the
  end (see **Output**). Each stream keeps at most 16 MiB; `truncated` is
  `true` when one was cut.
- The same checks, certificate and time limits as
  [`forge nodes terminal`](#forge-nodes-terminal) apply, except the idle
  timeout.
- The CLI can only prompt for a one-time code when stdin is a terminal. When
  you pipe input, verify first with [`forge auth step-up`](/docs/reference/cli/auth/#forge-auth-step-up).

:::caution
The command line is written to the audit log (`terminal.exec`, first 1000
characters). Pass secrets on stdin, never as arguments or in a `--shell`
command line.
:::

**Examples**

```sh
# Run one command
forge nodes exec <node> -- uptime

# Run a pipeline as a given login
forge nodes exec <node> --login <login> --shell -- 'journalctl -u nginx --since -1h | tail -n 50'

# Pass a secret on stdin instead of the command line
printf '%s' "$DB_PASSWORD" | forge nodes exec <node> -- sh -c 'cat > /etc/myapp/db-password'

# Fail a script when the remote command fails
forge nodes exec <node> --propagate-exit-code -- systemctl is-active nginx
```

**Output**

A command that succeeds:

```text
$ forge nodes exec node-01 -- uptime
 22:22:07 up 10:13,  2 users,  load average: 0.08, 0.08, 0.03
```

```json
{
  "result": {
    "exit_code": 0,
    "login": "root",
    "node": "node-01",
    "session": "365286e7038834c998d3eec380bb1dc2",
    "stderr": "",
    "stdout": " 22:22:07 up 10:13,  2 users,  load average: 0.08, 0.08, 0.03\n",
    "truncated": false
  },
  "schema": "forge.cli/v1"
}
```

A command that writes to both streams and exits with `3`. Without
`--propagate-exit-code`, `forge` itself exits `0`:

```text
$ forge nodes exec node-01 -- sh -c 'echo out; echo err >&2; exit 3'
out
err
forge: the remote command exited with status 3
```

```json
{
  "result": {
    "exit_code": 3,
    "login": "root",
    "node": "node-01",
    "session": "4bc57e95d8ebe92b1b503d1eab406ee2",
    "stderr": "err\n",
    "stdout": "out\n",
    "truncated": false
  },
  "schema": "forge.cli/v1"
}
```

`node` is the node's host name and `session` is the id that
[`forge sessions list`](/docs/reference/cli/sessions/#forge-sessions-list) shows.
When the node reports no status, `exit_code` is `255`.

Exit codes:

- `0`: the command ran, whatever its status. With
  `--propagate-exit-code`, only when it exited with `0`.
- `1`–`255` with `--propagate-exit-code`: the remote command's non-zero
  status.
- `1`: the session was revoked (code `revoked`), the command did not finish
  (code `session_ended`, `max_lifetime` and similar), or the server could
  not connect to the node.
- `2`: no command after `--`, an invalid login name, or an ambiguous name.
- `3`: not signed in.
- `4`: you lack `terminal.open` on the node.
- `5`: an MFA-verified session is needed and the CLI cannot prompt (run
  [`forge auth step-up`](/docs/reference/cli/auth/#forge-auth-step-up) first).
- `6`: no such node.
- `7`: the node has no SSH endpoint and did not report a host key.
- `9`: the server cannot be reached, or the connection broke.

:::note
With `--propagate-exit-code`, a remote status of `1`–`10` overlaps the CLI's
own exit codes. Use `-o json` and read `exit_code` when you need to tell
them apart.
:::

**See also**: [Browser terminal (SSH)](/docs/guides/terminal/#open-a-session-from-the-command-line),
[`forge sessions`](/docs/reference/cli/sessions/),
[`forge nodes terminal`](#forge-nodes-terminal).
