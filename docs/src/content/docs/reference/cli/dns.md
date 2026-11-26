---
title: forge dns
description: List the local names of nodes and services, with their addresses and published ports, and show or change the suffix they end in.
sidebar:
  order: 19
---

`forge dns` shows the [local names](/docs/concepts/local-names/) every node keeps
in its hosts file. To change a node's name, use
[`forge nodes dns-name`](/docs/reference/cli/nodes/#forge-nodes-dns-name).
Every command takes the [global flags](/docs/reference/cli/#global-flags).

## Commands

| Command | What it does |
| --- | --- |
| [`forge dns list`](#forge-dns-list) | Lists the names of nodes and services, with addresses and ports |
| [`forge dns suffix get`](#forge-dns-suffix-get) | Shows the suffix every name ends in and where it comes from |
| [`forge dns suffix set`](#forge-dns-suffix-set) | Sets the suffix of every local name |
| [`forge dns suffix reset`](#forge-dns-suffix-reset) | Clears the runtime setting and goes back to the config file's suffix |

### forge dns list

Lists every local name on nodes you can see: node names and service names,
the address other nodes use, the mesh address nodes in the same fabric use,
and the ports a service publishes on its node. Needs `view` on each node;
names on nodes you cannot see are left out.

**Synopsis**

```text
forge dns list [--workspace <WORKSPACE>] [--kind <KIND>] [--stack <STACK>]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--workspace <WORKSPACE>` | id | all workspaces you can see | no | Only this workspace's names | `--workspace <workspace-id>` |
| `--kind <KIND>` | `node` \| `service` | both | no | Only node names or only service names | `--kind service` |
| `--stack <STACK>` | stack id | all | no | Only that compose stack's `stack-service` and `replica` names; needs `view` on its environment | `--stack <stack-id>` |

**Examples**

```sh
# Every name
forge dns list

# Services and their ports, as JSON
forge dns list --kind service -o json
```

**Output**

Two nodes in the cluster `lab`, before any service is deployed:

```text
$ forge dns list
NAME             KIND  NODE   ADDRESS        MESH        PORTS
lab-b.lab.local  node  lab-b  192.0.2.11  10.64.0.16
lab-c.lab.local  node  lab-c  192.0.2.12  10.64.0.17
```

A service adds one row per node running it, for example
`api.shop.lab.local  service  lab-c  192.0.2.12  10.64.0.17  8080/tcp->80`.
With `-o json` each row has the fields of
[`GET /api/v1/names`](/docs/reference/rest-api/#local-names) `names[]`.

Columns: `NAME` (the name every node resolves), `KIND` (`node` or
`service`), `NODE` (the node's label; for a service, the node running it),
`ADDRESS` (the address Forge sees the node connect from), `MESH` (the
node's fabric address, used by nodes in the same fabric), `PORTS`
(`<node port>/<proto>-><container port>`).

Exit codes:

- `0`: success.
- `2`: `--kind` is not `node` or `service`.
- `3`: not signed in.
- `9`: the server is unreachable.

**See also**: [Rename a node and reach services by name](/docs/guides/local-names/),
[REST API: local names](/docs/reference/rest-api/#local-names).

### forge dns suffix get

Shows the suffix every local name ends in and where it comes from:
`setting` (the runtime setting), `config` (`[names] suffix` in
`forge.toml`) or `default` (`local`). Needs `view` on the workspace.

**Synopsis**

```text
forge dns suffix get
```

**Arguments and flags**

None besides the [global flags](/docs/reference/cli/#global-flags).

**Examples**

```sh
forge dns suffix get
forge dns suffix get -o json
```

**Output**

```text
$ forge dns suffix get
SUFFIX  SOURCE   CONFIG  DEFAULT
local   default  local   local
```

With `-o json` the object has the fields of
[`GET /api/v1/names/suffix`](/docs/reference/rest-api/#local-names).

Exit codes:

- `0`: success.
- `3`: not signed in.
- `9`: the server is unreachable.

### forge dns suffix set

Sets the suffix of every local name in the fleet. Every node gets its new
names on the next pass (30 seconds) and the old names stop resolving. Forge
containers follow on their next sync. Needs `system.update` on the platform
(**Platform admin**) and, when MFA is required, an MFA-verified session:
the CLI asks for a one-time code when the server answers
`step_up_required`. Audited as `names.suffix.update`.

**Synopsis**

```text
forge dns suffix set <suffix>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<suffix>` | string | none | yes | Lowercase DNS labels separated by dots, at most 63 characters. One label only from `local`, `internal`, `lan`, `home`, `test`, `localdomain`, `intranet`, `corp`; no `localhost` | `home.arpa` |

**Examples**

```sh
forge dns suffix set home.arpa
forge dns suffix set corp.internal
```

**Output**

```text
$ forge dns suffix set home.arpa
SUFFIX     SOURCE   CONFIG  DEFAULT
home.arpa  setting  local   local
```

A refused suffix prints the reason, for example:

```text
$ forge dns suffix set com
error: `com` could be a public top-level domain; use one of local, internal, lan, home, test, localdomain, intranet, corp or a domain you own (`corp.example.com`) [invalid_input]
```

Exit codes:

- `0`: success.
- `2`: the suffix is invalid (`400`).
- `3`: not signed in.
- `4`: you lack `system.update` on the platform.
- `5`: the session is not MFA-verified and no code was given (`step_up_required`).
- `9`: the server is unreachable.

### forge dns suffix reset

Clears the runtime setting. Names go back to `[names] suffix` from
`forge.toml`, or `local` when that is not set. Same permission, MFA and
audit as [`forge dns suffix set`](#forge-dns-suffix-set).

**Synopsis**

```text
forge dns suffix reset
```

**Arguments and flags**

None besides the [global flags](/docs/reference/cli/#global-flags).

**Examples**

```sh
forge dns suffix reset
```

**Output**

```text
$ forge dns suffix reset
SUFFIX  SOURCE   CONFIG  DEFAULT
local   default  local   local
```

Exit codes:

- `0`: success.
- `3`: not signed in.
- `4`: you lack `system.update` on the platform.
- `5`: the session is not MFA-verified and no code was given (`step_up_required`).
- `9`: the server is unreachable.
