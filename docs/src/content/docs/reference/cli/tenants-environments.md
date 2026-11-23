---
title: forge tenants and forge environments
description: List and create tenants, and list, create and move environments between clusters.
sidebar:
  order: 17
---

`forge tenants` and `forge environments` manage the workspace's tenancy:
tenants, and the environments that hold your services inside a cluster.
For the model, see [Clusters and nodes](/docs/concepts/clusters/); for the tasks,
see [Working inside a cluster](/docs/guides/clusters/). Roles can be bound on a
tenant or an environment; see [Roles and access](/docs/guides/roles-and-access/).
Every command takes the [global flags](/docs/reference/cli/#global-flags).

## Commands

| Command | What it does |
| --- | --- |
| [`forge tenants list`](#forge-tenants-list) | Lists the tenants of the workspace |
| [`forge tenants create`](#forge-tenants-create) | Creates a tenant |
| [`forge environments list`](#forge-environments-list) | Lists the environments you can see |
| [`forge environments create`](#forge-environments-create) | Creates an environment in a cluster |
| [`forge environments move`](#forge-environments-move) | Moves an environment, with its services, to another cluster |

The CLI cannot rename or delete tenants and environments.

### forge tenants list

Lists the tenants of the workspace. Needs `view` on the workspace.

**Synopsis**

```text
forge tenants list [OPTIONS]
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
# Every tenant
forge tenants list
# Tenant ids, for role bindings
forge -o json tenants list | jq -r '.result[] | "\(.id) \(.name)"'
```

**Output**

```text
$ forge tenants list
ID  NAME  CREATED
```

```json
$ forge -o json tenants list
{
  "result": [],
  "schema": "forge.cli/v1"
}
```

The capture comes from a system without tenants. Each element of `result` has
the shape shown under [`forge tenants create`](#forge-tenants-create).

Exit codes: `0` success; `3` not signed in; `4` permission denied; `9` server
unreachable.

**See also**: [`forge tenants create`](#forge-tenants-create),
[`forge access grant`](/docs/reference/cli/access/#forge-access-grant).

### forge tenants create

Creates a tenant. Needs `workspace.manage` on the workspace (**Admin** or
**Platform admin**). Audited as `tenant.create`.

**Synopsis**

```text
forge tenants create [OPTIONS] <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<name>` | string | — | yes | The tenant's name, 1 to 100 characters. Spaces around it are trimmed | `acme` |

**Examples**

```sh
# Create a tenant
forge tenants create acme
# Create a tenant and bind a role on it
TENANT_ID=$(forge -o json tenants create acme | jq -r '.result.id')
forge access grant <user> operator "tenant:$TENANT_ID"
```

**Output**

The table output prints the new tenant as a key/value list. With `-o json`:

```json
{
  "schema": "forge.cli/v1",
  "result": {
    "id": "<tenant-id>",
    "workspace_id": "<workspace-id>",
    "name": "acme",
    "created_at": 1794554342323
  }
}
```

Shape from the server's schema; not captured on a live system.

Exit codes: `0` created; `2` the name is empty or longer than 100
characters; `3` not signed in; `4` permission denied; `9` server
unreachable.

**See also**: [`forge tenants list`](#forge-tenants-list),
[Roles and access](/docs/guides/roles-and-access/).

### forge environments list

Lists the environments of the workspace that you can see. Environments you
cannot view are left out.

**Synopsis**

```text
forge environments list [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |
| `--cluster <CLUSTER>` | cluster id, name or abbreviation | none | no | Only environments in this cluster | `--cluster lab` |
| `--project <PROJECT>` | project id or name | none | no | Only environments of this project | `--project shop` |

**Examples**

```sh
# Every environment you can see
forge environments list
# Environments of one cluster
forge environments list --cluster lab
# Names of one project's environments
forge -o json environments list --project shop | jq -r '.result[].name'
```

**Output**

```text
$ forge environments list
ID                                    NAME  PROJECT                               CLUSTER                               PROTECTED
01900000-0000-7000-8000-000000000018  test  01900000-0000-7000-8000-000000000060  01900000-0000-7000-8000-000000000005  false
```

```json
$ forge -o json environments list
{
  "result": [
    {
      "cluster_id": "01900000-0000-7000-8000-000000000005",
      "created_at": 1794554498041,
      "id": "01900000-0000-7000-8000-000000000018",
      "name": "test",
      "project_id": "01900000-0000-7000-8000-000000000060",
      "protected": false,
      "workspace_id": "01900000-0000-7000-8000-000000000001"
    }
  ],
  "schema": "forge.cli/v1"
}
```

`PROJECT` and `CLUSTER` are ids. `project_id` is the project the environment
belongs to.

Exit codes: `0` success; `3` not signed in; `9` server unreachable.

**See also**: [`forge environments create`](#forge-environments-create),
[`forge clusters list`](/docs/reference/cli/clusters/#forge-clusters-list).

### forge environments create

Creates an environment of a project in a cluster. Names
are unique within a cluster, so another cluster can have an environment with the
same name. Needs `workspace.manage` on the target cluster and on the
project (**Admin** or **Platform admin**). Audited as `environment.create`.
Create the project first with
[`forge projects create`](/docs/reference/cli/projects/#forge-projects-create),
or in the console under **Projects**.

**Synopsis**

```text
forge environments create [OPTIONS] --cluster <CLUSTER> --project <PROJECT> <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<name>` | string | — | yes | The environment's name: free text, 1 to 100 characters, such as `test`, `staging` or `prod`. Spaces around it are trimmed | `staging` |
| `--cluster <CLUSTER>` | cluster id, name or abbreviation | — | yes | The cluster the environment lives in | `--cluster lab` |
| `--project <PROJECT>` | project id or name | — | yes | The project this environment belongs to | `--project shop` |
| `--protected` | flag | off | no | Protect the environment from deletion | `--protected` |

**Examples**

```sh
# Create the test environment of the project shop in the cluster lab
forge environments create test --cluster lab --project shop
# Create a protected production environment
forge environments create prod --cluster edge-eu --project shop --protected
```

**Output**

The table output prints the new environment as a key/value list. With `-o json`,
`result` has the shape of one element of
[`forge environments list`](#forge-environments-list).

Shape from the server's schema; not captured on a live system.

Exit codes: `0` created; `2` an invalid name, an ambiguous cluster or
project name, or a cluster or project of another workspace; `3` not
signed in; `4` permission denied; `6` no such cluster or project; `7` the
cluster already has an environment with that name; `9` server unreachable.

**See also**: [`forge environments move`](#forge-environments-move),
[`forge projects create`](/docs/reference/cli/projects/#forge-projects-create),
[`forge services create`](/docs/reference/cli/services/#forge-services-create),
[`forge stacks create`](/docs/reference/cli/stacks/#forge-stacks-create),
[Create an environment](/docs/guides/clusters/#create-an-environment).

### forge environments move

Moves an environment, with its services, to another cluster. An environment
always stays in a cluster. Needs `workspace.manage` on the environment and on
the target cluster. Audited as `environment.move`.

**Synopsis**

```text
forge environments move [OPTIONS] <ENVIRONMENT> <CLUSTER>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<environment>` | environment id or name | — | yes | The environment to move. A name is matched without regard to case; when several clusters have an environment with that name, use the id | `staging` |
| `<cluster>` | cluster id, name or abbreviation | — | yes | The target cluster | `edge-eu` |
| `--yes` | flag | off | no | Skip the confirmation prompt | `--yes` |

:::note
A role on a cluster covers the cluster's nodes, not the environments in it.
Moving an environment does not change who can deploy to it. See
[Authorization](/docs/concepts/authorization/#cluster-scope-and-environment-scope).
:::

**Examples**

```sh
# Move an environment, with a confirmation prompt
forge environments move staging edge-eu
# Move by id without prompting
forge environments move <environment-id> <cluster-id> --yes
```

**Output**

The table output prints the moved environment as a key/value list. With
`-o json`, `result` has the shape of one element of
[`forge environments list`](#forge-environments-list), with the new `cluster_id`.

Shape from the server's schema; not captured on a live system.

Exit codes: `0` moved; `1` you did not confirm; `2` an ambiguous environment or
cluster name, a cluster of another workspace, or no `--yes` where no prompt
is possible; `3` not signed in; `4` permission denied; `6` no such environment
or cluster; `7` the target cluster already has an environment with that name;
`9` server unreachable.

**See also**: [`forge clusters delete`](/docs/reference/cli/clusters/#forge-clusters-delete),
[Move an environment to another cluster](/docs/guides/clusters/#move-an-environment-to-another-cluster).
