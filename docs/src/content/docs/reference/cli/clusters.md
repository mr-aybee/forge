---
title: forge clusters
description: List, show, create, update and delete clusters, save and apply a cluster's ingress profile, and read its live ingress state and routing table.
sidebar:
  order: 18
---

`forge clusters` manages clusters, the named groups of nodes that services
and sites run on, and each cluster's ingress profile: the Nginx layout on
its edge nodes. For the model, see [Clusters and nodes](/docs/concepts/clusters/);
for the tasks, see [Working inside a cluster](/docs/guides/clusters/) and
[Ingress](/docs/guides/ingress/).
Every command takes the [global flags](/docs/reference/cli/#global-flags).

## Permissions

| Commands | Cedar action | Scope | Roles | MFA |
| --- | --- | --- | --- | --- |
| `list` | `view` | the workspace, then each cluster | Every role | no |
| `get`, `live`, `routing`, `ingress get` | `view` | the cluster | Every role | no |
| `live` (per edge) | `server.manage` | each edge; edges without it are left out | **Operator**, **Admin**, **Platform admin** | no |
| `create` | `workspace.manage` | the workspace | **Admin**, **Platform admin** | no |
| `update`, `delete`, `ingress set` | `workspace.manage` | the cluster | **Admin**, **Platform admin** | no |
| `ingress apply`, `ingress remove` | `view` and `proxy.manage` | the cluster; each edge of the profile | **Operator**, **Admin**, **Platform admin** | yes |
| `ingress set`, `ingress apply` with custom Nginx directives (`nginx.custom`) | `server.command` | each edge of the profile | **Operator**, **Admin**, **Platform admin** | yes |

On a terminal the CLI asks for your authenticator code when a command needs
an MFA-verified session. With `--non-interactive` it exits with `5`; run
[`forge auth step-up`](/docs/reference/cli/auth/#forge-auth-step-up) first. See
the [Cedar schema](/docs/reference/cedar-schema/).

## Commands

| Command | What it does |
| --- | --- |
| [`forge clusters list`](#forge-clusters-list) | Lists the clusters you can see, with node counts |
| [`forge clusters get`](#forge-clusters-get) | Shows one cluster |
| [`forge clusters create`](#forge-clusters-create) | Creates a cluster from a JSON spec |
| [`forge clusters update`](#forge-clusters-update) | Changes a cluster's name, description, badge and environment link |
| [`forge clusters delete`](#forge-clusters-delete) | Deletes a cluster that holds no environments |
| [`forge clusters live`](#forge-clusters-live) | Asks each ingress edge node for its live state |
| [`forge clusters routing`](#forge-clusters-routing) | Shows the cluster's endpoints, edges and resolved upstreams |
| [`forge clusters ingress get`](#forge-clusters-ingress-get) | Shows the cluster's ingress profile and what each edge last reported |
| [`forge clusters ingress set`](#forge-clusters-ingress-set) | Saves the ingress profile from `--edge` and `--mode`, from a JSON file, or both |
| [`forge clusters ingress apply`](#forge-clusters-ingress-apply) | Pushes the saved profile to its edge nodes |
| [`forge clusters ingress remove`](#forge-clusters-ingress-remove) | Takes the ingress configuration off the edge nodes |

Where a command takes `<cluster>`, give the cluster's id, its name or its
abbreviation. Names and abbreviations are matched without regard to case
and must match exactly one cluster.

### forge clusters list

Lists the clusters of the workspace that you can see, with how many nodes
each has and how many are online. Needs `view` on the workspace; clusters you
cannot view are left out.

**Synopsis**

```text
forge clusters list [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

**Examples**

```sh
# Every cluster you can see
forge clusters list
# Clusters with a node offline
forge -o json clusters list | jq -r '.result[] | select(.nodes_online < .node_count) | .name'
```

**Output**

```text
$ forge clusters list
ID                                    NAME  ABBR  DESCRIPTION
01900000-0000-7000-8000-000000000005  lab   LB    Lab cluster (node-01, node-02)
```

```json
$ forge -o json clusters list
{
  "result": [
    {
      "abbr": "LB",
      "created_at": 1794554342323,
      "description": "Lab cluster (node-01, node-02)",
      "icon": null,
      "id": "01900000-0000-7000-8000-000000000005",
      "name": "lab",
      "environment_id": null,
      "node_count": 2,
      "nodes_online": 2,
      "workspace_id": "01900000-0000-7000-8000-000000000001"
    }
  ],
  "schema": "forge.cli/v1"
}
```

Exit codes: `0` success; `3` not signed in; `4` permission denied; `9` server
unreachable.

**See also**: [`forge clusters get`](#forge-clusters-get),
[`forge nodes list`](/docs/reference/cli/nodes/#forge-nodes-list).

### forge clusters get

Shows one cluster. Needs `view` on the cluster.

**Synopsis**

```text
forge clusters get [OPTIONS] <CLUSTER>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<cluster>` | cluster id, name or abbreviation | — | yes | The cluster to show | `lab` |

**Examples**

```sh
# Show the cluster
forge clusters get lab
# The cluster's id, for other commands
forge -o json clusters get lab | jq -r '.result.id'
```

**Output**

```text
$ forge clusters get lab
{
  "abbr": "LB",
  "created_at": 1794554342323,
  "description": "Lab cluster (node-01, node-02)",
  "icon": null,
  "id": "01900000-0000-7000-8000-000000000005",
  "name": "lab",
  "environment_id": null,
  "workspace_id": "01900000-0000-7000-8000-000000000001"
}
```

```json
$ forge -o json clusters get lab
{
  "result": {
    "abbr": "LB",
    "created_at": 1794554342323,
    "description": "Lab cluster (node-01, node-02)",
    "icon": null,
    "id": "01900000-0000-7000-8000-000000000005",
    "name": "lab",
    "environment_id": null,
    "workspace_id": "01900000-0000-7000-8000-000000000001"
  },
  "schema": "forge.cli/v1"
}
```

`get` has no `node_count` or `nodes_online`; use
[`forge clusters list`](#forge-clusters-list) for those.

Exit codes: `0` success; `2` the name or abbreviation matches more than one cluster; `3` not
signed in; `4` permission denied; `6` no such cluster; `9` server
unreachable.

**See also**: [`forge clusters list`](#forge-clusters-list),
[`forge clusters update`](#forge-clusters-update).

### forge clusters create

Creates an empty cluster from a JSON spec. Add nodes and environments to it
afterwards. Needs `workspace.manage` on the workspace (**Admin** or
**Platform admin**). Audited as `cluster.create`.

**Synopsis**

```text
forge clusters create [OPTIONS] --file <FILE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--file <FILE>` | path, `@<path>` or `@-` | — | yes | The [cluster spec](#cluster-spec) as JSON. `@-` reads it from stdin | `--file cluster.json` |

**Examples**

```sh
# Create from a file
forge clusters create --file cluster.json
# Create from stdin and keep the new id
echo '{"name":"edge-eu","abbr":"EU"}' | forge -o json clusters create --file @- | jq -r '.result.id'
```

**Output**

The table output prints the new cluster as a key/value list. With `-o json`,
`result` has the shape shown under [`forge clusters get`](#forge-clusters-get).

Shape from the server's schema; not captured on a live system.

Exit codes: `0` created; `2` invalid spec (`invalid_body`, a bad name,
abbreviation or image, or an environment of another workspace); `3` not signed
in; `4` permission denied; `9` server unreachable.

**See also**: [`forge environments create`](/docs/reference/cli/tenants-environments/#forge-environments-create),
[Create a cluster](/docs/guides/clusters/#create-a-cluster).

### forge clusters update

Changes a cluster's name, description, environment link and badge from a JSON
spec. Needs `workspace.manage` on the cluster. Audited as `cluster.update`.

**Synopsis**

```text
forge clusters update [OPTIONS] --file <FILE> <CLUSTER>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<cluster>` | cluster id, name or abbreviation | — | yes | The cluster to change | `lab` |
| `--file <FILE>` | path, `@<path>` or `@-` | — | yes | The [cluster spec](#cluster-spec) as JSON | `--file cluster.json` |
| `--yes` | flag | off | no | Skip the confirmation prompt | `--yes` |

:::caution
`name`, `description` and `environment_id` are replaced: leaving out
`description` clears it, and leaving out `environment_id` unlinks the
environment. Only `abbr` and `icon` keep their current value when you leave
them out. Start from `forge -o json clusters get <cluster> | jq '.result'`.
:::

**Examples**

```sh
# Rename the cluster, with a confirmation prompt
forge clusters update lab --file cluster.json
# Change only the description, keeping the rest, without prompting
forge -o json clusters get lab \
  | jq '.result | {name, description: "Lab cluster", environment_id}' \
  | forge clusters update lab --file @- --yes
```

**Output**

The table output prints the updated cluster as a key/value list. With
`-o json`, `result` has the shape shown under
[`forge clusters get`](#forge-clusters-get).

Shape from the server's schema; not captured on a live system.

Exit codes: `0` updated; `1` you did not confirm; `2` invalid spec, an
ambiguous name, or no `--yes` where no prompt is possible; `3` not signed in;
`4` permission denied; `6` no such cluster; `9` server unreachable.

**See also**: [`forge clusters get`](#forge-clusters-get),
[Set the cluster badge](/docs/guides/clusters/#set-the-cluster-badge).

### forge clusters delete

Deletes a cluster, with its ingress profile and cluster network
configuration. A cluster that still holds environments cannot be deleted; move
them first with
[`forge environments move`](/docs/reference/cli/tenants-environments/#forge-environments-move).
Needs `workspace.manage` on the cluster. Audited as `cluster.delete`.

**Synopsis**

```text
forge clusters delete [OPTIONS] <CLUSTER>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<cluster>` | cluster id, name or abbreviation | — | yes | The cluster to delete | `edge-eu` |
| `--yes` | flag | off | no | Skip the confirmation prompt | `--yes` |

:::caution
Deleting a cluster removes its ingress profile and its cluster-network
settings from Forge. Move or remove what runs on it first.
:::

**Examples**

```sh
# Delete, with a confirmation prompt
forge clusters delete edge-eu
# Delete without prompting
forge clusters delete edge-eu --yes
```

**Output**

The table output prints nothing. With `-o json`:

```json
{ "schema": "forge.cli/v1", "result": null }
```

Shape from the server's schema; not captured on a live system.

Exit codes: `0` deleted; `1` you did not confirm; `2` an ambiguous name, or
no `--yes` where no prompt is possible; `3` not signed in; `4` permission
denied; `6` no such cluster; `7` the cluster still has environments (the
message names them); `9` server unreachable.

**See also**: [`forge environments move`](/docs/reference/cli/tenants-environments/#forge-environments-move),
[Delete a cluster](/docs/guides/clusters/#delete-a-cluster).

### forge clusters live

Asks each ingress edge node in the cluster's ingress profile for its live
ingress state: Nginx and Keepalived, and the virtual IPs the edge holds.
Each edge is an `agent.command` job; the
server waits up to 15 seconds per edge. Needs `view` on the cluster. Edges on
which you lack `server.manage` are left out.

**Synopsis**

```text
forge clusters live [OPTIONS] <CLUSTER>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<cluster>` | cluster id, name or abbreviation | — | yes | The cluster to ask | `lab` |

**Examples**

```sh
# Live state of every edge
forge clusters live lab
# Edges whose status check did not succeed
forge -o json clusters live lab | jq '.result.edges | map_values(select(.status != "succeeded"))'
```

**Output**

```text
$ forge clusters live lab
{
  "edges": {}
}
```

```json
$ forge -o json clusters live lab
{
  "result": {
    "edges": {}
  },
  "schema": "forge.cli/v1"
}
```

The capture comes from a cluster without an ingress profile, so `edges` is
empty. With edges, each key is an edge node id (shape from the server's
schema):

```json
{
  "edges": {
    "<edge-node-id>": { "status": "succeeded", "error": null, "state": { "…": "…" } }
  }
}
```

`status` is `succeeded`, `failed` (with `error`) or `running` when the edge
did not answer within 15 seconds.

Exit codes: `0` success; `2` an ambiguous name; `3` not signed in; `4`
permission denied; `6` no such cluster; `9` server unreachable.

**See also**: [`forge clusters routing`](#forge-clusters-routing),
[`forge clusters ingress get`](#forge-clusters-ingress-get),
[Ingress](/docs/guides/ingress/).

### forge clusters routing

Shows the cluster's routing table: every site served from the cluster's
ingress profile, its edges with their status, and its targets with the
upstream addresses they resolve to now. Needs `view` on the cluster.

**Synopsis**

```text
forge clusters routing [OPTIONS] <CLUSTER>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<cluster>` | cluster id, name or abbreviation | — | yes | The cluster to show | `lab` |

**Examples**

```sh
# The routing table
forge clusters routing lab
# Domains and the upstream addresses behind them
forge -o json clusters routing lab \
  | jq -r '.result.endpoints[] | "\(.domains[0]) -> \([.targets[].upstreams[].address] | join(", "))"'
```

**Output**

```text
$ forge clusters routing lab
SITE   DOMAINS             TLS  SOURCE              EDGES  UPSTREAMS
xcweb  xcweb.example.test  off  edges (not linked)  lab-a  192.0.2.11:8080 (lab-b), 192.0.2.12:8080 (lab-c)
```

`SOURCE` is `profile` for sites linked to the cluster's ingress profile, and
`edges (not linked)` for sites with explicit edge nodes in this cluster. When
the cluster has no ingress profile, a line under the table says so; save one
with [`forge clusters ingress set`](#forge-clusters-ingress-set). A cluster
without sites prints `no sites route through this cluster`.

```json
$ forge -o json clusters routing lab
{
  "result": {
    "endpoints": []
  },
  "schema": "forge.cli/v1"
}
```

The capture comes from a cluster without sites. With sites, each endpoint has
this shape (from the server's schema):

```json
{
  "id": "<site-id>",
  "name": "shop",
  "domains": ["shop.example.com"],
  "tls": "acme",
  "edges": [
    { "server_id": "<edge-node-id>", "name": "<edge-node-name>", "address": "192.0.2.10", "status": "applied" }
  ],
  "source": "profile",
  "linked": true,
  "targets": [
    {
      "target": { "kind": "app", "service_id": "<service-id>", "port": 8080 },
      "label": "shop",
      "internal_name": "shop.<environment>.<cluster-domain>",
      "standby": false,
      "upstreams": [
        { "address": "10.88.0.2:8080", "resolved": ["10.88.0.2:8080"], "standby": false, "server_id": "<node-id>", "node": "<node-name>", "state": "…" }
      ]
    }
  ]
}
```

Only typed `targets` appear under `targets`; plain `upstreams` of a site do
not. `source` is `profile` or `edges`, and `linked` is `true` for sites
linked to the ingress profile. The answer also has a top-level `profile`
flag: whether the cluster has an ingress profile.

Exit codes: `0` success; `2` an ambiguous name; `3` not signed in; `4`
permission denied; `6` no such cluster; `9` server unreachable.

**See also**: [`forge sites get`](/docs/reference/cli/sites-certificates/#forge-sites-get),
[Check routing](/docs/guides/ingress/#check-routing-and-traffic),
[Cluster network](/docs/guides/cluster-network/).

### forge clusters ingress get

Shows the cluster's ingress profile and what each edge node last reported.
`forge clusters ingress show` is an alias. Needs `view` on the cluster.

**Synopsis**

```text
forge clusters ingress get [OPTIONS] <CLUSTER>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<cluster>` | cluster id, name or abbreviation | — | yes | The cluster | `lab` |

**Examples**

```sh
# The layout and the state of each edge
forge clusters ingress get lab
# Save the profile to a file, to edit it and send it back with `set --file`
forge -o json clusters ingress get lab | jq '.result.profile' > profile.json
# Was the saved profile applied?
forge -o json clusters ingress get lab | jq '.result.status.dirty == false'
```

**Output**

Shape from the server's schema; not captured on a live system. The table
output is a summary:

```text
cluster     lab
mode        active_standby
failover    dns
applied     yes
applied_at  2026-11-14 18:20:25
edges:
  NODE   ROLE     PUBLIC_IP     STATUS   ERROR
  lab-b  primary  203.0.113.10  applied  -
  lab-c  backup   203.0.113.11  applied  -
```

| Line | Meaning |
| --- | --- |
| `mode` | `standalone`, `active_standby` or `active_active` |
| `failover` | `dns` or `vrrp` |
| `virtual_ips` | The VRRP virtual IPs. Shown only when the profile has some |
| `applied` | `yes` when the saved profile is the one on the edges. After a `set` or a `remove` it reads ``no (run `forge clusters ingress apply`)`` |
| `applied_at` | When the profile was last applied. Left out before the first apply |
| `edges` | One row per edge of the profile. `STATUS` is `applied`, `failed`, `pending` or `removed` from the last apply or remove, and `not applied` for an edge that has no result yet |

A cluster without a profile prints:

```text
cluster  lab
profile  none (save one with `forge clusters ingress set`)
```

With `-o json`, `result` is the API answer:

```json
{
  "result": {
    "cluster": { "id": "<cluster-id>", "name": "lab", "abbr": "LB", … },
    "profile": {
      "ingress": {
        "mode": "active_standby",
        "edges": [
          { "server_id": "<node-id>", "role": "primary", "public_ip": "203.0.113.10" },
          { "server_id": "<node-id>", "role": "backup", "public_ip": "203.0.113.11" }
        ],
        "failover": "dns",
        "vrrp": null,
        "nginx": {
          "worker_connections": 4096,
          "keepalive_timeout_s": 65,
          "proxy_read_timeout_s": 300,
          "client_max_body_mb": 25,
          "gzip": true,
          "server_tokens": false,
          "hsts": true,
          "real_ip_from": [],
          "rate_limit": 0,
          "access_log": "combined",
          "custom": ""
        }
      }
    },
    "status": {
      "dirty": false,
      "applied_at": 1794590425000,
      "edges": {
        "<node-id>": { "status": "applied", "error": null, "job_id": "<job-id>", "updated_at": 1794590425000 }
      }
    },
    "members": [
      { "id": "<node-id>", "name": "lab-b", "hostname": "lab-b", "role": "worker", "status": "online", "os": "linux", "address": "192.0.2.11" }
    ]
  },
  "schema": "forge.cli/v1"
}
```

| Field | Description |
| --- | --- |
| `cluster` | The cluster, as in [`forge clusters get`](#forge-clusters-get) |
| `profile` | The saved [ingress profile](#ingress-profile), or `null` when the cluster has none |
| `status.dirty` | `true` when the profile was saved, or edges were removed, since the last apply |
| `status.applied_at` | When the profile was last applied, in epoch milliseconds |
| `status.edges` | The result of the last apply or remove per edge node id: `status`, `error`, `job_id`, `updated_at` |
| `status.dns` | What Forge published in DNS at the last apply. Present only for a profile with `dns` |
| `members` | The nodes that can be edges: the cluster's nodes and the control node. `address` is the address Forge reaches the node on |

Exit codes: `0` success; `2` an ambiguous name; `3` not signed in; `4`
permission denied; `6` no such cluster; `9` server unreachable.

**See also**: [`forge clusters ingress set`](#forge-clusters-ingress-set),
[`forge clusters live`](#forge-clusters-live), [Ingress](/docs/guides/ingress/).

### forge clusters ingress set

Saves the cluster's ingress profile. It does not touch the nodes: run
[`forge clusters ingress apply`](#forge-clusters-ingress-apply) next. Asks
first unless you pass `--yes`. Needs `workspace.manage` on the cluster
(**Admin** or **Platform admin**). A profile with custom Nginx directives
(`nginx.custom`) also needs `server.command` on every edge, with an
MFA-verified session. Audited as `cluster.profile.update`.

There are two ways to give the profile, and you can combine them:

| You pass | What is saved |
| --- | --- |
| `--edge` and/or `--mode` | The saved profile with its edges and/or mode replaced. Its other settings stay: failover, VRRP, Nginx tuning and DNS. A cluster without a profile gets a new one with the defaults |
| `--file` | The file, as the whole [ingress profile](#ingress-profile). Settings the file leaves out go back to their defaults |
| `--file` with `--edge` and/or `--mode` | The file, with its edges and/or mode replaced by the flags |

Pass at least one of `--file`, `--edge` and `--mode`; otherwise the command
exits with `2`.

**Synopsis**

```text
forge clusters ingress set [OPTIONS] <CLUSTER>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<cluster>` | cluster id, name or abbreviation | — | yes | The cluster | `lab` |
| `--file <FILE>` | path, `@<path>` or `@-` | none | no | The whole [ingress profile](#ingress-profile) as JSON, `{"ingress": {…}}`. `@-` reads stdin | `--file profile.json` |
| `--edge <EDGE>` | `<node>[:primary\|backup][=<public-ip>]`; repeatable | none | no | An edge node. `<node>` is a node id, host name or display name and must be a member of the cluster. The list replaces the profile's edges. Without a role, the first `--edge` is `primary` and the others are `backup`. `<public-ip>` is the IPv4 or IPv6 address clients reach the edge on | `--edge lab-b:primary=203.0.113.10` |
| `--mode <MODE>` | enum: `standalone`, `active_standby`, `active_active` | the saved mode; `standalone` for a single edge | no | How the edges share the traffic. Several edges and no saved mode need `--mode` (exit `2`) | `--mode active_standby` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

The server checks the profile and answers a broken rule with exit `2`:

| Mode | Rule |
| --- | --- |
| `standalone` (**Single edge** in the console) | Exactly one edge |
| `active_standby` | At least two edges, exactly one of them `primary` |
| `active_active` | At least two edges |
| every mode | No edge twice; each edge is a member of the cluster; each `public_ip` is an IP address |
| `failover: vrrp` (not standalone) | Needs `vrrp` with virtual IPs, which only `--file` can set |
| `dns` | Every edge needs a `public_ip` |

**Examples**

```sh
# One edge: a standalone profile
forge clusters ingress set lab --edge lab-b --yes

# Two edges, lab-b primary, with their public addresses
forge clusters ingress set lab --edge lab-b=203.0.113.10 --edge lab-c=203.0.113.11 --mode active_standby --yes

# Make lab-c the primary and keep everything else
forge clusters ingress set lab --edge lab-b:backup=203.0.113.10 --edge lab-c:primary=203.0.113.11 --yes

# Switch the saved profile to active-active
forge clusters ingress set lab --mode active_active --yes

# The whole profile from a file
forge clusters ingress set lab --file profile.json --yes

# Change one Nginx setting: read, edit, send back
forge -o json clusters ingress get lab \
  | jq '.result.profile | .ingress.nginx.client_max_body_mb = 100' \
  | forge clusters ingress set lab --file @- --yes
```

**Output**

The table output prints one line on stderr, then the answer:

```text
Saved. Run `forge clusters ingress apply lab` to push it to the edge nodes.
saved  yes
```

```json
{
  "result": {
    "saved": true
  },
  "schema": "forge.cli/v1"
}
```

Exit codes: `0` saved; `1` you did not answer `yes`; `2` none of `--file`,
`--edge` and `--mode`, a bad `--edge`, several edges without a mode, no
edge at all, a profile the server refuses (the rules above), an invalid
file (`invalid_body`), an ambiguous name, or no `--yes` where no prompt is
possible; `3` not signed in; `4` no `workspace.manage` on the cluster, or
no `server.command` on an edge for custom directives; `5` an MFA-verified
session is needed for custom directives; `6` no such cluster or node; `9`
server unreachable.

**See also**: [`forge clusters ingress apply`](#forge-clusters-ingress-apply),
[Ingress profile](#ingress-profile),
[Set up a cluster edge group](/docs/guides/ingress/#set-up-a-cluster-edge-group).

### forge clusters ingress apply

Pushes the saved profile to its edge nodes: on each edge Forge writes the
global Nginx configuration, checks it with `nginx -t` and reloads Nginx, and
configures keepalived when the profile uses VRRP. A profile with `dns` also
publishes its DNS records. Asks first unless you pass `--yes`. Audited as
`cluster.profile.apply`.

Needs `view` on the cluster and `proxy.manage` on every edge (**Operator**,
**Admin** or **Platform admin**), with an MFA-verified session. A profile
with custom Nginx directives also needs `server.command` on every edge.

Each edge is an `agent.command` job. The server waits up to 120 seconds per
edge, one edge after the other.

**Synopsis**

```text
forge clusters ingress apply [OPTIONS] <CLUSTER>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<cluster>` | cluster id, name or abbreviation | — | yes | The cluster | `lab` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

:::caution
Applying reloads Nginx on every edge. When `nginx -t` rejects the new
configuration on an edge, that edge keeps its previous file and reports
`failed`.
:::

**Examples**

```sh
# Apply, with a confirmation prompt
forge clusters ingress apply lab
# Apply in a script and stop when an edge failed
forge clusters ingress apply lab --yes || exit 1
```

**Output**

Shape from the server's schema; not captured on a live system.

```text
NODE   STATUS   ERROR  JOB
lab-b  applied  -      <job-id>
lab-c  applied  -      <job-id>
```

`STATUS` is `applied`, `failed` (with `ERROR`) or `pending` when the edge
did not answer within 120 seconds; follow a pending edge with
[`forge jobs watch <job-id>`](/docs/reference/cli/jobs/#forge-jobs-watch).

```json
{
  "result": {
    "edges": {
      "<node-id>": { "status": "applied", "error": null, "job_id": "<job-id>", "updated_at": 1794590425000 },
      "<node-id>": { "status": "applied", "error": null, "job_id": "<job-id>", "updated_at": 1794590425000 }
    },
    "dns": null
  },
  "schema": "forge.cli/v1"
}
```

`edges` is keyed by edge node id. `dns` is what Forge published, or `null`
for a profile without `dns`.

When an edge failed, the command exits with `10` and the error code
`ingress_failed`. The table output prints the table first, then the error
on stderr:

```text
NODE   STATUS   ERROR               JOB
lab-b  applied  -                   <job-id>
lab-c  failed   nginx: test failed  <job-id>
error: apply of the ingress of cluster lab failed: lab-c: nginx: test failed [ingress_failed]
```

With `-o json`, stdout then holds only the
[error document](/docs/reference/cli/#exit-codes).

Exit codes: `0` every edge is `applied` or `pending`; `1` you did not
answer `yes`; `2` the cluster has no saved profile or the profile has no
edge, an ambiguous name, or no `--yes` where no prompt is possible; `3` not
signed in; `4` no `view` on the cluster, no `proxy.manage` on an edge, or
no `server.command` for custom directives; `5` an MFA-verified session is
needed; `6` no such cluster; `9` server unreachable; `10` an edge failed
(`ingress_failed`).

**See also**: [`forge clusters ingress get`](#forge-clusters-ingress-get),
[`forge clusters live`](#forge-clusters-live),
[`forge sites apply`](/docs/reference/cli/sites-certificates/#forge-sites-apply).

### forge clusters ingress remove

Takes the ingress configuration off the cluster's edge nodes. An edge that
reports `removed` leaves the profile, so the node can then move to another
cluster. An edge that failed, or did not answer in time, stays in the
profile so you can run the command again. The profile's other settings
(mode, Nginx tuning, VRRP, DNS) are kept. Asks first unless you pass
`--yes`. Audited as `cluster.profile.remove`.

Needs `view` on the cluster and `proxy.manage` on every edge, with an
MFA-verified session.

**Synopsis**

```text
forge clusters ingress remove [OPTIONS] <CLUSTER>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<cluster>` | cluster id, name or abbreviation | — | yes | The cluster | `lab` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

:::caution
The sites served from these edges stop answering.
:::

**Examples**

```sh
# Remove, with a confirmation prompt
forge clusters ingress remove lab
# Remove in a script
forge clusters ingress remove lab --yes
```

**Output**

Shape from the server's schema; not captured on a live system.

```text
NODE   STATUS   ERROR  JOB
lab-b  removed  -      <job-id>
lab-c  removed  -      <job-id>
```

`STATUS` is `removed`, `failed` (with `ERROR`) or `pending`.

```json
{
  "result": {
    "edges": {
      "<node-id>": { "status": "removed", "error": null, "job_id": "<job-id>", "updated_at": 1794590425000 }
    }
  },
  "schema": "forge.cli/v1"
}
```

When an edge failed, the command exits with `10` (`ingress_failed`) after
the table; with `-o json`, stdout holds only the error document.

Exit codes: `0` every edge is `removed` or `pending`; `1` you did not
answer `yes`; `2` the cluster has no profile or the profile has no edge, an
ambiguous name, or no `--yes` where no prompt is possible; `3` not signed
in; `4` no `view` on the cluster or no `proxy.manage` on an edge; `5` an
MFA-verified session is needed; `6` no such cluster; `9` server
unreachable; `10` an edge failed (`ingress_failed`).

**See also**: [`forge clusters ingress get`](#forge-clusters-ingress-get),
[Ingress](/docs/guides/ingress/).

## Cluster spec

`forge clusters create` and `forge clusters update` read a JSON spec from
`--file`. `--file` takes a path, `@<path>`, or `@-` to read from stdin. The
CLI checks the body against the server's schema before sending and exits with
`2` (`invalid_body`) on a mismatch; see [`forge api`](/docs/reference/cli/api/).

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `name` | string | — (required) | 1 to 63 characters: letters, digits, space, `-`, `_` and `.` |
| `description` | string | `""` | Free text; the server keeps the first 500 characters |
| `environment_id` | id or `null` | `null` | An environment of the same workspace to link the cluster to |
| `abbr` | string | create: none. update: keep the current value | Badge letters: up to 4 letters or digits, stored in upper case. `""` clears it |
| `icon` | string | create: none. update: keep the current value | Badge image as a base64 data URL (`data:image/png;base64,…`; PNG, JPEG, WebP, GIF or SVG), at most 128 KB. `""` removes it |

Example `cluster.json`:

```json
{
  "name": "edge-eu",
  "description": "Edge nodes in the EU region",
  "abbr": "EU"
}
```

## Ingress profile

[`forge clusters ingress set --file`](#forge-clusters-ingress-set) reads the
whole profile as JSON, and
[`forge clusters ingress get`](#forge-clusters-ingress-get) returns it under
`profile`. The profile is one object with an `ingress` key. Unknown fields
under `ingress` are refused with exit `2`. For what each setting does on the
edges, see [Ingress](/docs/guides/ingress/).

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `ingress.mode` | `standalone`, `active_standby` or `active_active` | — (required) | How the edges share the traffic |
| `ingress.edges[]` | list | — (required, at least one) | The edge nodes |
| `ingress.edges[].server_id` | node id | — (required) | A member of the cluster. `forge clusters ingress get` lists them under `members` |
| `ingress.edges[].role` | `primary` or `backup` | `primary` | `active_standby` needs exactly one `primary` |
| `ingress.edges[].public_ip` | IPv4 or IPv6 address | none | The address clients reach this edge on. Required on every edge when `dns` is set |
| `ingress.failover` | `dns` or `vrrp` | `dns` | How clients find the edges. `vrrp` moves virtual IPs between edges with keepalived and needs the edges on one LAN. Ignored for `standalone` |
| `ingress.vrrp` | object or `null` | `null` | Required when `failover` is `vrrp` and the mode is not `standalone` |
| `ingress.vrrp.vips` | list of IPv4 addresses, `a.b.c.d` or `a.b.c.d/nn` (`/8` to `/32`) | — | The virtual IPs: exactly one for `active_standby`, one per edge for `active_active` |
| `ingress.vrrp.interface` | string, up to 15 characters | — | The interface that carries the virtual IP, such as `eth0` |
| `ingress.vrrp.router_id` | integer, 1 to 255 | — | The first VRRP router id; each further virtual IP uses the next one |
| `ingress.nginx.worker_connections` | integer, 64 to 1000000 | `4096` | Nginx `worker_connections` |
| `ingress.nginx.keepalive_timeout_s` | integer, seconds, 0 to 3600 | `65` | Nginx `keepalive_timeout` |
| `ingress.nginx.proxy_read_timeout_s` | integer, seconds, 1 to 86400 | `300` | Nginx `proxy_read_timeout` |
| `ingress.nginx.client_max_body_mb` | integer, MB, 1 to 10240 | `25` | Nginx `client_max_body_size` |
| `ingress.nginx.gzip` | bool | `true` | Compress responses |
| `ingress.nginx.server_tokens` | bool | `false` | Show the Nginx version |
| `ingress.nginx.hsts` | bool | `true` | Send `Strict-Transport-Security` on HTTPS |
| `ingress.nginx.real_ip_from` | list of IPv4 addresses or CIDRs | `[]` | Proxies in front of the edges whose `X-Forwarded-For` is trusted |
| `ingress.nginx.rate_limit` | integer, requests per second per client IP | `0` (off) | A global rate limit. Sites can add their own |
| `ingress.nginx.access_log` | `combined`, `json` or `off` | `combined` | The access log format |
| `ingress.nginx.custom` | string | `""` | Directives added at `http {}` level. Needs `server.command` (MFA) on every edge. See [Custom blocks](/docs/guides/ingress/#custom-configuration) for the allowed directives |
| `ingress.dns` | object | none | Lets Forge keep DNS records for the endpoints on a PowerDNS node |
| `ingress.dns.server_id` | node id | — (required in `dns`) | The PowerDNS node that holds the zones |
| `ingress.dns.ttl` | integer, seconds, 30 to 86400 | `60` | The record TTL. Lower values fail over faster |
| `ingress.dns.auto_failover` | bool | `true` | Health-check the edges and move records off failed ones |

:::caution
`ingress.nginx.custom` runs inside Nginx as root on every edge. Saving or
applying a profile with a non-empty block needs `server.command` with an
MFA-verified session on every edge.
:::

Example `profile.json`: two edges, active-standby, with DNS failover and a
larger upload limit.

```json
{
  "ingress": {
    "mode": "active_standby",
    "edges": [
      { "server_id": "<node-id>", "role": "primary", "public_ip": "203.0.113.10" },
      { "server_id": "<node-id>", "role": "backup", "public_ip": "203.0.113.11" }
    ],
    "failover": "dns",
    "nginx": {
      "client_max_body_mb": 100,
      "access_log": "json"
    },
    "dns": {
      "server_id": "<powerdns-node-id>",
      "ttl": 60,
      "auto_failover": true
    }
  }
}
```

Example with a virtual IP on one LAN:

```json
{
  "ingress": {
    "mode": "active_standby",
    "edges": [
      { "server_id": "<node-id>", "role": "primary" },
      { "server_id": "<node-id>", "role": "backup" }
    ],
    "failover": "vrrp",
    "vrrp": { "vips": ["192.0.2.100/24"], "interface": "eth0", "router_id": 51 }
  }
}
```

## Docker Swarm

A cluster created with `"mode": "swarm"` runs its services on Docker Swarm
(see [Docker Swarm](/docs/guides/docker-swarm/)). These commands manage its
members; they need `swarm.manage` on the cluster, with MFA.

### forge clusters swarm status

Members, live Swarm nodes and manager quorum of a Swarm cluster

**Synopsis**

```text
forge clusters swarm status [OPTIONS] <CLUSTER>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<cluster>` | string | - | yes | Cluster id or name | - |

**Examples**

```sh
# Swarm state, managers and members of a cluster
forge clusters swarm status <cluster>
# The same as JSON
forge -o json clusters swarm status <cluster>
```

### forge clusters swarm join

Add a node to the cluster's swarm; the first node creates it

**Synopsis**

```text
forge clusters swarm join [OPTIONS] <CLUSTER> <NODE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<cluster>` | string | - | yes | Cluster id or name | - |
| `<node>` | string | - | yes | Node id, host name or name | - |
| `--role` | enum: manager \| worker | `worker` | no | The first node is always a manager. Keep 1, 3 or 5 managers | - |
| `--advertise-addr` | string | - | no | Address or interface other nodes reach this one on | - |
| `--addr-pool` | string | - | no | Overlay address pool (CIDR), first node only; repeat for more | - |
| `--autolock` | flag | - | no | Lock manager keys at rest, first node only | - |
| `--wait` | flag | - | no | Wait for the job to finish (exit 10 when it fails) | - |
| `--timeout` | integer | `900` | no | Seconds `--wait` waits | - |

**Examples**

```sh
# Start the swarm on the first node (always a manager)
forge clusters swarm join <cluster> lab-a --advertise-addr 192.0.2.10 --wait
# Add a worker and wait up to 10 minutes
forge clusters swarm join <cluster> lab-b --wait --timeout 600
# Add a second manager
forge clusters swarm join <cluster> lab-c --role manager --wait
```

### forge clusters swarm leave

Remove a node from the swarm

**Synopsis**

```text
forge clusters swarm leave [OPTIONS] <CLUSTER> <NODE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<cluster>` | string | - | yes | Cluster id or name | - |
| `<node>` | string | - | yes | Node id, host name or name | - |
| `--force` | flag | - | no | Leave even when the node is unreachable or the last manager | - |
| `--yes` | flag | - | no | Skip the confirmation prompt | - |
| `--wait` | flag | - | no | Wait for the job to finish (exit 10 when it fails) | - |
| `--timeout` | integer | `900` | no | Seconds `--wait` waits | - |

**Examples**

```sh
# Take a node out of the swarm
forge clusters swarm leave <cluster> lab-b --yes --wait
# Force a node that cannot leave cleanly
forge clusters swarm leave <cluster> lab-b --force --yes
```

### forge clusters swarm node

Change a member's role, availability or labels

**Synopsis**

```text
forge clusters swarm node [OPTIONS] <CLUSTER> <NODE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<cluster>` | string | - | yes | Cluster id or name | - |
| `<node>` | string | - | yes | Node id, host name or name | - |
| `--role` | enum: manager \| worker | - | no | New role: manager or worker | - |
| `--availability` | string | - | no | active, pause or drain | - |
| `--label` | flag | - | no | <KEY=VALUE> Replace the node's labels with these (`KEY=VALUE`, repeat) | - |
| `--clear-labels` | flag | - | no | Remove every label | - |

**Examples**

```sh
# Drain a member before maintenance
forge clusters swarm node <cluster> lab-b --availability drain
# Promote a worker to manager
forge clusters swarm node <cluster> lab-c --role manager
# Replace the member's labels
forge clusters swarm node <cluster> lab-b --label zone=a --label disk=ssd
```

### forge clusters swarm rotate-tokens

Rotate the swarm's join tokens

**Synopsis**

```text
forge clusters swarm rotate-tokens [OPTIONS] <CLUSTER>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<cluster>` | string | - | yes | Cluster id or name | - |
| `--worker` | flag | - | no | Rotate the worker join token | - |
| `--manager` | flag | - | no | Rotate the manager join token | - |
| `--yes` | flag | - | no | Skip the confirmation prompt | - |

**Examples**

```sh
# Rotate the worker join token
forge clusters swarm rotate-tokens <cluster> --worker --yes
# Rotate both join tokens
forge clusters swarm rotate-tokens <cluster> --worker --manager --yes
```

