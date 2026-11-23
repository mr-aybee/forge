---
title: forge projects
description: List, show, create, rename and delete projects, the products whose environments hold your services, from the command line.
sidebar:
  order: 8.5
---

`forge projects` manages the console's **Projects**: the products
you run, each with its environments. It uses the
[project endpoints](/docs/reference/rest-api/#projects). For the
model, see
[Projects, environments and services](/docs/concepts/projects-environments-services/).
Every command takes the [global flags](/docs/reference/cli/#global-flags).

| Level | API | CLI |
| --- | --- | --- |
| **Project** | `/api/v1/projects` | `forge projects` (this page) |
| **Environment** | `/api/v1/environments` | [`forge environments`](/docs/reference/cli/tenants-environments/) |
| **Service** | `/api/v1/services` | [`forge services`](/docs/reference/cli/services/) |

`<project>` is a project id or name. Names match without regard to
case. A name that matches more than one project exits with `2`
(`ambiguous`) and lists the ids; a name that matches none exits with `6`.

## Permissions

| Commands | Cedar action | Scope | Roles | MFA |
| --- | --- | --- | --- | --- |
| `list`, `show` | `view` | the project | Every role bound on the project or the workspace | no |
| `create` | `workspace.manage` | the workspace | **Admin**, **Platform admin** | no |
| `update`, `delete` | `workspace.manage` | the project | **Admin**, **Platform admin** | no |

`list` and `show` include only the environments you may `view`, and
`service_count` counts the services of those environments. See the
[Cedar schema](/docs/reference/cedar-schema/).

## Commands

| Command | What it does |
| --- | --- |
| [`forge projects list`](#forge-projects-list) | Lists the projects you can see, with their environments and service counts |
| [`forge projects show`](#forge-projects-show) | Shows one project with its environments |
| [`forge projects create`](#forge-projects-create) | Creates a project |
| [`forge projects update`](#forge-projects-update) | Renames a project or changes its description |
| [`forge projects delete`](#forge-projects-delete) | Deletes a project that has no environments |

### forge projects list

Lists the projects you can see, each with its environments and the
number of services in them. Needs `view` on each project; others are
left out.

**Synopsis**

```text
forge projects list [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

**Examples**

```sh
# Every project you can see
forge projects list
# Projects without an environment
forge -o json projects list | jq -r '.result[] | select(.environments | length == 0) | .name'
```

**Output**

Shape from the server's schema; not captured on a live system.

```text
ID                                    NAME     ENVIRONMENTS         SERVICES  DESCRIPTION
01900000-0000-7000-8000-000000000060  shop     shop-prod, shop-uat  3         Web shop
01900000-0000-7000-8000-000000000061  billing  -                    0         -
```

`ENVIRONMENTS` is the names of the project's environments, joined with
commas, or `-` when it has none.

```json
{
  "result": [
    {
      "id": "01900000-0000-7000-8000-000000000060",
      "workspace_id": "01900000-0000-7000-8000-000000000001",
      "name": "shop",
      "description": "Web shop",
      "created_at": 1794590000000,
      "environments": [
        {
          "id": "01900000-0000-7000-8000-000000000018",
          "name": "shop-prod",
          "cluster_id": "01900000-0000-7000-8000-000000000005",
          "protected": true
        },
        {
          "id": "01900000-0000-7000-8000-000000000019",
          "name": "shop-uat",
          "cluster_id": "01900000-0000-7000-8000-000000000005",
          "protected": false
        }
      ],
      "service_count": 3,
      "environment_names": "shop-prod, shop-uat"
    }
  ],
  "schema": "forge.cli/v1"
}
```

| Field | Type | Description |
| --- | --- | --- |
| `id` | id | The project's id |
| `workspace_id` | id | The workspace it belongs to |
| `name` | string | The name, unique in the workspace |
| `description` | string | Free text; `""` when not set |
| `created_at` | integer, epoch milliseconds | When it was created |
| `environments[]` | list | The environments you may view: `id`, `name`, `cluster_id` (`null` for an environment outside a cluster) and `protected` |
| `service_count` | integer | Services across those environments |
| `environment_names` | string | Added by the CLI for the table: the environment names, or `-` |

Exit codes: `0` success; `3` not signed in; `9` server unreachable.

**See also**: [`forge projects show`](#forge-projects-show),
[`forge environments list`](/docs/reference/cli/tenants-environments/#forge-environments-list),
[`forge services list`](/docs/reference/cli/services/#forge-services-list).

### forge projects show

Shows one project with its environments. `forge projects get` is an
alias. Needs `view` on the project.

**Synopsis**

```text
forge projects show [OPTIONS] <PROJECT>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<project>` | project id or name | — | yes | The project to show | `shop` |

**Examples**

```sh
# Show a project by name
forge projects show shop
# Its id, for other commands
forge -o json projects show shop | jq -r .result.id
```

**Output**

Shape from the server's schema; not captured on a live system. The key/value
view leaves out empty fields, such as an empty description.

```text
id             01900000-0000-7000-8000-000000000060
workspace_id   01900000-0000-7000-8000-000000000001
name           shop
description    Web shop
created_at     2026-11-14 18:20:25
environments:
  ID                                    NAME       CLUSTER_ID                            PROTECTED
  01900000-0000-7000-8000-000000000018  shop-prod  01900000-0000-7000-8000-000000000005  yes
  01900000-0000-7000-8000-000000000019  shop-uat   01900000-0000-7000-8000-000000000005  no
service_count  3
```

With `-o json`, `result` is one element of
[`forge projects list`](#forge-projects-list), without
`environment_names`.

Exit codes: `0` success; `2` the name matches more than one project
(`ambiguous`); `3` not signed in; `6` no project with that id or name
that you can see; `9` server unreachable.

**See also**: [`forge projects list`](#forge-projects-list),
[`forge projects update`](#forge-projects-update).

### forge projects create

Creates a project in the workspace. It does not ask first. A new
project has no environments; add them with
[`forge environments create`](/docs/reference/cli/tenants-environments/#forge-environments-create),
then add services with
[`forge services create`](/docs/reference/cli/services/#forge-services-create). Needs
`workspace.manage` on the workspace (**Admin** or **Platform admin**).
Audited as `project.create`.

**Synopsis**

```text
forge projects create [OPTIONS] <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<name>` | string | — | yes | The name: 1 to 63 lower-case letters, digits and dashes. Unique in the workspace | `shop` |
| `--description <DESCRIPTION>` | string | `""` | no | What the project is, for people. The server keeps the first 500 characters | `--description "Web shop"` |

**Examples**

```sh
# Create a project
forge projects create shop --description "Web shop"
# Then give it a production environment in the cluster lab
forge environments create shop-prod --cluster lab --project shop
# Create one in a script and keep its id
PROJECT_ID=$(forge -o json projects create billing | jq -r .result.id)
```

**Output**

The new project. Shape from the server's schema; not captured on a live
system.

```text
id            01900000-0000-7000-8000-000000000060
workspace_id  01900000-0000-7000-8000-000000000001
name          shop
description   Web shop
created_at    2026-11-14 18:20:25
```

```json
{
  "result": {
    "id": "01900000-0000-7000-8000-000000000060",
    "workspace_id": "01900000-0000-7000-8000-000000000001",
    "name": "shop",
    "description": "Web shop",
    "created_at": 1794590000000,
    "environments": [],
    "service_count": 0
  },
  "schema": "forge.cli/v1"
}
```

Exit codes: `0` created; `2` an invalid name; `3` not signed in; `4` no
`workspace.manage` on the workspace; `7` the workspace already has a
project with that name; `9` server unreachable.

**See also**: [`forge environments create`](/docs/reference/cli/tenants-environments/#forge-environments-create),
[`forge services create`](/docs/reference/cli/services/#forge-services-create),
[Services](/docs/guides/services/).

### forge projects update

Renames a project or changes its description. Only the fields you pass
change. Asks first unless you pass `--yes`. Its environments and their
services stay attached: they refer to the project by id. Needs
`workspace.manage` on the project. Audited as `project.update`.

**Synopsis**

```text
forge projects update [OPTIONS] <PROJECT>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<project>` | project id or name | — | yes | The project to change | `shop` |
| `--name <NAME>` | string | keep the current name | one of `--name` and `--description` | The new name: 1 to 63 lower-case letters, digits and dashes | `--name store` |
| `--description <DESCRIPTION>` | string | keep the current description | one of `--name` and `--description` | The new description. `""` clears it. The server keeps the first 500 characters | `--description "Online store"` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

**Examples**

```sh
# Rename a project (asks for confirmation)
forge projects update shop --name store
# Change the description in a script
forge projects update shop --description "Online store" --yes
# Clear the description
forge projects update shop --description "" --yes
```

**Output**

The updated project, in the shape shown under
[`forge projects show`](#forge-projects-show).

Exit codes: `0` updated; `1` you did not answer `yes`; `2` neither `--name`
nor `--description`, an invalid name, an ambiguous name, or no `--yes` where
no prompt is possible; `3` not signed in; `4` no `workspace.manage` on the
project; `6` no such project; `7` the workspace already has a
project with the new name; `9` server unreachable.

**See also**: [`forge projects show`](#forge-projects-show).

### forge projects delete

Deletes a project that has no environments. Asks first unless you pass
`--yes`. Needs `workspace.manage` on the project. Audited as
`project.delete`.

:::note
The CLI cannot delete an environment. Delete the project's environments
in the console (**Projects**, the environment's row menu, **Delete…**) first; see [Services](/docs/guides/services/).
:::

**Synopsis**

```text
forge projects delete [OPTIONS] <PROJECT>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<project>` | project id or name | — | yes | The project to delete | `billing` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

**Examples**

```sh
# Delete a project (asks for confirmation)
forge projects delete billing
# Delete it in a script
forge projects delete billing --yes
```

**Output**

```text
deleted  01900000-0000-7000-8000-000000000061
name     billing
```

```json
{
  "result": {
    "deleted": "01900000-0000-7000-8000-000000000061",
    "name": "billing"
  },
  "schema": "forge.cli/v1"
}
```

`deleted` is the id of the project that was removed.

Exit codes: `0` deleted; `1` you did not answer `yes`; `2` an ambiguous
name, or no `--yes` where no prompt is possible; `3` not signed in; `4` no
`workspace.manage` on the project; `6` no such project; `7` the
project still has environments (the message names them); `9` server
unreachable.

**See also**: [`forge projects list`](#forge-projects-list),
[`forge environments list`](/docs/reference/cli/tenants-environments/#forge-environments-list).
