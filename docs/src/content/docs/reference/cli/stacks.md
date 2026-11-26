---
title: forge stacks
description: Create, update, deploy, roll back, take down and inspect Docker Compose stacks from the command line.
sidebar:
  order: 8
---

`forge stacks` manages Compose stacks through the same
[stack endpoints](/docs/reference/rest-api/#compose-stacks) as the console. For
background, read [Compose stacks](/docs/concepts/compose-stacks/); for a walkthrough,
see [Deploy a compose stack](/docs/guides/compose-stacks/). Every command takes the
[global flags](/docs/reference/cli/#global-flags).

`<stack>` is a stack id or name. A name must match exactly one stack you can
see. When the name exists in several environments, the command exits with `2`
(`ambiguous`) and lists the ids; pass `--environment` (id or name) or the id.
`--node` takes a node id, hostname or display name.

The outputs on this page come from a throwaway stack `docs-demo` on a test
lab. It was created, deployed, restarted, taken down and deleted for the
capture.

## Permissions

| Commands | Cedar action | Scope | MFA |
| --- | --- | --- | --- |
| `list`, `show`, `revisions`, `revision`, `logs`, `refresh` | `view` | the stack's environment | no |
| `create`, `update`, `delete`, `deploy`, `rollback`, `service`, `down` | `stack.manage` | the stack's environment | no |
| `create` (also with `--dry-run`) | `server.manage` | the node in `--node` | no |
| `deploy`, `rollback` | `server.manage` | each node the stack deploys to | no |
| `deploy`, `rollback` of a stack with host-access settings | `stack.privileged` | the stack's environment | yes (step-up) |
| `plan`, `apply` | `stack.manage` | the document's environment | no |
| `plan`, `apply` | `server.manage` | each node the stack runs on | no |
| `apply` of a stack with host-access settings | `stack.privileged` | the document's environment | yes (step-up) |
| `validate` | none (offline) | — | no |

`stack.manage` is in the `platform_admin`, `workspace_admin`, `operator` and
`developer` roles. `stack.privileged` is in `platform_admin` and
`workspace_admin` only. See the [Cedar schema](/docs/reference/cedar-schema/).

## Commands

| Command | What it does |
| --- | --- |
| [`forge stacks list`](#forge-stacks-list) | Lists the stacks you can see |
| [`forge stacks show`](#forge-stacks-show) | Shows one stack: compose file, policy preview and service state per node |
| [`forge stacks revisions`](#forge-stacks-revisions) | Lists the saved revisions of a stack |
| [`forge stacks revision`](#forge-stacks-revision) | Prints the compose file of one saved revision |
| [`forge stacks create`](#forge-stacks-create) | Creates a stack from a compose file, or previews it with `--dry-run` |
| [`forge stacks update`](#forge-stacks-update) | Saves a new revision from a compose file and/or `.env` file |
| [`forge stacks deploy`](#forge-stacks-deploy) | Deploys the latest or a given revision on the stack's nodes |
| [`forge stacks rollback`](#forge-stacks-rollback) | Deploys an older revision |
| [`forge stacks service`](#forge-stacks-service) | Starts, stops, restarts or pulls services on one node |
| [`forge stacks logs`](#forge-stacks-logs) | Prints the last log lines of one service on one node |
| [`forge stacks refresh`](#forge-stacks-refresh) | Reads the service state from every node again |
| [`forge stacks down`](#forge-stacks-down) | Stops and removes the stack's containers |
| [`forge stacks delete`](#forge-stacks-delete) | Deletes a stack that is down on every node |
| [`forge stacks validate`](#forge-stacks-validate) | Checks a `forge.stack.yaml` and the files it names, offline |
| [`forge stacks plan`](#forge-stacks-plan) | Shows what applying a `forge.stack.yaml` would do |
| [`forge stacks apply`](#forge-stacks-apply) | Creates or updates a stack from a `forge.stack.yaml` and rolls it out |

### forge stacks list

Lists the Compose stacks you can see, with their latest and deployed revision
and the state of their nodes. Needs `view` on each stack's environment; stacks
in other environments are left out.

**Synopsis**

```text
forge stacks list [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--environment <ENVIRONMENT>` | environment id or name | all environments | no | Only stacks in this environment | `--environment lab` |
| `--node <NODE>` | node id, hostname or display name | all nodes | no | Only stacks with services on this node, or created from it | `--node web-01` |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

**Examples**

```sh
# Every stack you can see
forge stacks list
# Stacks in one environment
forge stacks list --environment <environment>
# Stacks that run on, or were created from, one node
forge stacks list --node <node-name>
```

**Output**

```text
ID                                    NAME       KIND     REVISION  DEPLOYED  NODES  STATUS
01900000-0000-7000-8000-000000000017  docs-demo  compose  1         0         0      -
```

`DEPLOYED` is `0` until the first deploy. `STATUS` lists the distinct node
states, comma-separated, or `-` when the stack runs on no node.

```json
{
  "result": [
    {
      "created_at": 1794590549602,
      "created_by": {
        "id": "01900000-0000-7000-8000-000000000003",
        "type": "user"
      },
      "default_server": "01900000-0000-7000-8000-000000000006",
      "deployed_revision": 0,
      "dir": null,
      "env_keys": [],
      "id": "01900000-0000-7000-8000-000000000017",
      "kind": "compose",
      "name": "docs-demo",
      "environment_id": "01900000-0000-7000-8000-000000000018",
      "node_count": 0,
      "node_status": "-",
      "nodes": [],
      "revision": 1,
      "updated_at": 1794590549602,
      "workspace_id": "01900000-0000-7000-8000-000000000001"
    }
  ],
  "schema": "forge.cli/v1"
}
```

Exit codes: `0` success; `2` the environment or node name is ambiguous; `3` not
signed in; `6` no environment or node with that name.

**See also**: [`forge stacks show`](#forge-stacks-show),
[Deploy a compose stack](/docs/guides/compose-stacks/).

### forge stacks show

Shows one stack: the compose file of its latest revision, the folder on the
node, the policy preview and the service state per node. `forge stacks get`
is an alias. Needs `view` on the stack's environment.

**Synopsis**

```text
forge stacks show [OPTIONS] <STACK>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<stack>` | stack id or name | — | yes | The stack | `docs-demo` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab` |

**Examples**

```sh
# One stack by name
forge stacks show <stack>
# A name that exists in several environments
forge stacks show <stack> --environment <environment>
# Only the compose file, for a script
forge -o json stacks show <stack> | jq -r .result.content
```

**Output**

The table output prints the same object as a key/value list, without the
envelope. `preview.blocked` is `true` when a policy finding stops a deploy;
`preview.privileged` is `true` when the stack uses host-access settings.

```json
{
  "result": {
    "content": "services:\r\n  hello:\r\n    image: alpine:3.20\r\n    command: [\"sleep\", \"3600\"]\r\n",
    "dir": "/opt/forge/stacks/lab/docs-demo",
    "nodes": [],
    "preview": {
      "blocked": false,
      "findings": [],
      "nodes": [
        {
          "findings": [],
          "hostname": "node-02",
          "internal_ip": "10.64.0.17",
          "ports": [],
          "server_id": "01900000-0000-7000-8000-000000000006",
          "services": [
            "hello"
          ]
        }
      ],
      "privileged": false,
      "services": [
        {
          "command": [
            "sleep",
            "3600"
          ],
          "depends_on": [],
          "entrypoint": null,
          "env_file": null,
          "environment": [],
          "expose": null,
          "findings": [],
          "healthcheck": null,
          "image": "alpine:3.20",
          "name": "hello",
          "nodes": [
            {
              "hostname": "node-02",
              "server_id": "01900000-0000-7000-8000-000000000006"
            }
          ],
          "policy": "allowed",
          "ports": [],
          "restart": null,
          "volumes": []
        }
      ]
    },
    "stack": {
      "created_at": 1794590549602,
      "deployed_revision": 0,
      "id": "01900000-0000-7000-8000-000000000017",
      "name": "docs-demo",
      …
    }
  },
  "schema": "forge.cli/v1"
}
```

Exit codes: `0` success; `2` ambiguous name; `3` not signed in; `4` no
`view` on the environment; `6` no stack with that name or id.

**See also**: [`forge stacks revisions`](#forge-stacks-revisions),
[`forge stacks refresh`](#forge-stacks-refresh).

### forge stacks revisions

Lists the saved revisions of a stack, newest first. Needs `view` on the
stack's environment.

**Synopsis**

```text
forge stacks revisions [OPTIONS] <STACK>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<stack>` | stack id or name | — | yes | The stack | `docs-demo` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab` |

**Examples**

```sh
# Revisions of a stack
forge stacks revisions <stack>
# Revision numbers and notes only
forge -o json stacks revisions <stack> | jq '.result[] | {revision, note}'
```

**Output**

```text
REVISION  SHA256                                                        AUTHOR                                                       AT             NOTE
1         749217a517d28a4157035934c061c9a66c06324b8c30b03cc426cb9500b…  {"id":"01900000-0000-7000-8000-000000000003","type":"user"}  1794590549602  -
```

`AT` is epoch milliseconds. The list leaves `content` empty; read one
revision's file with [`forge stacks revision`](#forge-stacks-revision).

```json
{
  "result": [
    {
      "author": {
        "id": "01900000-0000-7000-8000-000000000003",
        "type": "user"
      },
      "content": "",
      "created_at": 1794590549602,
      "note": null,
      "revision": 1,
      "sha256": "749217a517d28a4157035934c061c9a66c06324b8c30b03cc426cb9500bd9e28",
      "stack_id": "01900000-0000-7000-8000-000000000017"
    }
  ],
  "schema": "forge.cli/v1"
}
```

Exit codes: `0` success; `2` ambiguous name; `3` not signed in; `4` no
`view` on the environment; `6` no such stack.

**See also**: [`forge stacks revision`](#forge-stacks-revision),
[`forge stacks rollback`](#forge-stacks-rollback).

### forge stacks revision

Prints the compose file of one saved revision. Needs `view` on the stack's
environment.

**Synopsis**

```text
forge stacks revision [OPTIONS] <STACK> <REVISION>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<stack>` | stack id or name | — | yes | The stack | `docs-demo` |
| `<revision>` | integer | — | yes | The revision number | `1` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab` |

**Examples**

```sh
# Print revision 1
forge stacks revision <stack> 1
# Compare two revisions
diff <(forge stacks revision <stack> 1) <(forge stacks revision <stack> 2)
```

**Output**

The table output is the compose file as it was saved:

```text
services:
  hello:
    image: alpine:3.20
    command: ["sleep", "3600"]
```

With `-o json`, `result` is the revision object with the same fields as in
[`forge stacks revisions`](#forge-stacks-revisions), and `content` holds the
file.

Exit codes: `0` success; `2` ambiguous name or a revision that is not a
number; `3` not signed in; `4` no `view` on the environment; `6` no such stack
or revision.

**See also**: [`forge stacks revisions`](#forge-stacks-revisions).

### forge stacks create

Creates a stack from a compose file and saves it as revision 1. It does not
deploy it; run [`forge stacks deploy`](#forge-stacks-deploy) next. With
`--dry-run`, it validates the file and prints the placement and policy plan
without saving anything. Needs `stack.manage` on the environment and
`server.manage` on the node in `--node`, also for `--dry-run`.

**Synopsis**

```text
forge stacks create [OPTIONS] --environment <ENVIRONMENT> --node <NODE> --compose <COMPOSE> <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<name>` | string: `a-z`, `0-9`, `-`, `_`, up to 63 characters | — | yes | Stack name, unique in the environment | `docs-demo` |
| `--environment <ENVIRONMENT>` | environment id or name | — | yes | Environment of the stack | `--environment lab` |
| `--node <NODE>` | node id, hostname or display name | — | yes | Node the stack is created on. Services without placement run here | `--node web-01` |
| `-f`, `--compose <COMPOSE>` | path, or `-` for stdin | — | yes | Compose file | `-f compose.yaml` |
| `--env-file <ENV_FILE>` | path | none | no | `.env` file for the stack | `--env-file .env` |
| `--dir <DIR>` | absolute path without `..` | Forge's stack folder | no | Folder on the node for the compose project | `--dir /srv/app` |
| `--note <NOTE>` | string | none | no | Note saved with revision 1. Ignored with `--dry-run` | `--note "first version"` |
| `--dry-run` | flag | off | no | Validate and show the placement and policy plan; save nothing | `--dry-run` |

Forge stores the `.env` variables encrypted and shows only their names
(`env_keys`). Services that compose extensions place on other nodes are listed
per node in the preview; see [Compose extensions and policy](/docs/reference/compose-extensions/).

**Examples**

```sh
# Check a compose file without saving it
forge stacks create <name> --environment <environment> --node <node-name> -f compose.yaml --dry-run
# Create the stack with its .env file
forge stacks create <name> --environment <environment> --node <node-name> -f compose.yaml --env-file .env
# Read the compose file from stdin, in a folder you choose
cat compose.yaml | forge stacks create <name> --environment <environment> --node <node-name> -f - --dir /srv/<name>
```

**Output**

With `--dry-run`, the plan. `blocked` is `true` when a finding stops the
stack; `can_privileged` tells you whether you may deploy host-access
settings:

```json
{
  "blocked": false,
  "can_privileged": true,
  "findings": [],
  "nodes": [
    {
      "findings": [],
      "hostname": "node-02",
      "internal_ip": "10.64.0.17",
      "ports": [],
      "server_id": "01900000-0000-7000-8000-000000000006",
      "services": [
        "hello"
      ]
    }
  ],
  "privileged": false,
  "services": [
    {
      "command": [
        "sleep",
        "3600"
      ],
      "depends_on": [],
      "entrypoint": null,
      …
    }
  ]
}
```

Without `--dry-run`, the new stack and its preview:

```json
{
  "preview": {
    "blocked": false,
    "findings": [],
    "nodes": [ … ],
    "privileged": false,
    "services": [ … ]
  },
  "stack": {
    "deployed_revision": 0,
    "id": "01900000-0000-7000-8000-000000000017",
    "name": "docs-demo",
    "revision": 1,
    …
  }
}
```

With `-o json`, the same object is in `result`.

Exit codes: `0` success; `2` the compose or `.env` file cannot be read, an
invalid name, folder or compose file, a blocked policy finding, or a node in another
organization; `3` not signed in; `4` no `stack.manage` or `server.manage`;
`6` no such environment or node; `7` a stack with this name exists in the
environment.

**See also**: [`forge stacks deploy`](#forge-stacks-deploy),
[Deploy a compose stack](/docs/guides/compose-stacks/).

### forge stacks update

Saves a new revision from a new compose file and/or `.env` file. It does not
deploy it; run [`forge stacks deploy`](#forge-stacks-deploy) next. A compose
file identical to the latest revision, with the same `.env` variables, saves
no new revision. A changed `.env` file alone saves a new revision, so the
next deploy or apply reaches every node. Needs `stack.manage` on the
environment.

**Synopsis**

```text
forge stacks update [OPTIONS] <STACK>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<stack>` | stack id or name | — | yes | The stack | `docs-demo` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab` |
| `-f`, `--compose <COMPOSE>` | path, or `-` for stdin | keep the current file | one of `-f` or `--env-file` | New compose file | `-f compose.yaml` |
| `--env-file <ENV_FILE>` | path | keep the stored variables | one of `-f` or `--env-file` | New `.env` file. It replaces all stored variables | `--env-file .env` |
| `--note <NOTE>` | string | none | no | Note saved with the new revision | `--note "$CI_COMMIT_SHA"` |

**Examples**

```sh
# Save a new compose file as the next revision
forge stacks update <stack> -f compose.yaml --note "bump image"
# Replace only the environment variables
forge stacks update <stack> --env-file .env
```

A CI job that ships a new compose file:

```sh
forge stacks update <stack> --environment <environment> -f compose.yaml --note "$CI_COMMIT_SHA"
forge stacks deploy <stack> --environment <environment> --wait --yes --non-interactive
```

**Output**

The stack and the preview of the new revision, the same shape as
[`forge stacks create`](#forge-stacks-create): `{ "stack": { … }, "preview": { … } }`.
`stack.revision` is the new latest revision. Shape from the server's schema;
not captured on a live system.

Exit codes: `0` success; `2` a file cannot be read, neither `-f` nor
`--env-file` given, an invalid compose or `.env` file, a blocked policy
finding, or an ambiguous name; `3` not signed in; `4` no `stack.manage`; `6`
no such stack; `7` (`stale_version`) someone else saved the stack between the
CLI reading it and saving: run the command again to apply your change on top.

**See also**: [`forge stacks deploy`](#forge-stacks-deploy),
[`forge stacks revisions`](#forge-stacks-revisions).

### forge stacks deploy

Deploys the latest revision, or the one in `--revision`, on the stack's
nodes. It starts one `stack.action` job per node. When you deploy all
services, nodes that no longer run any service get a job that takes the stack
down there. It asks first unless you pass `--yes`. Needs `stack.manage` on the
environment and `server.manage` on each node; a stack with host-access
settings also needs `stack.privileged` and an MFA-verified session.

**Synopsis**

```text
forge stacks deploy [OPTIONS] <STACK>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<stack>` | stack id or name | — | yes | The stack | `docs-demo` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab` |
| `--revision <REVISION>` | integer | the latest revision | no | Revision to deploy. An older one rolls back | `--revision 3` |
| `--services <SERVICES>` | list | all services | no | Services to start, with the services they depend on | `--services web,worker` |
| `--pull` | flag | off | no | Pull images again even when present | `--pull` |
| `--wait` | flag | off | no | Wait for the jobs and print them when they finish | `--wait` |
| `--wait-timeout-secs <WAIT_TIMEOUT_SECS>` | integer, seconds | `1800` | no | How long `--wait` waits. The jobs keep running after a timeout | `--wait-timeout-secs 600` |
| `--yes` | flag | off | no | Skip the confirmation prompt | `--yes` |

**Examples**

```sh
# Deploy the latest revision and wait for the result
forge stacks deploy <stack> --wait --yes
# Deploy two services and pull their images again
forge stacks deploy <stack> --services web,worker --pull --yes
# CI: never prompt, fail on a failed job
forge stacks deploy <stack> --environment <environment> --wait --yes --non-interactive
```

**Output**

With `--wait`, the finished jobs:

```text
JOB                                   KIND          STATUS
01900000-0000-7000-8000-000000000019  stack.action  succeeded
```

Without `--wait`, the same columns for the jobs it started, usually with
status `queued`. Follow them with [`forge jobs watch`](/docs/reference/cli/jobs/#forge-jobs-watch).
With `-o json`, `result` is an array of
[job objects](/docs/reference/cli/jobs/#forge-jobs-get).

Exit codes: `0` success; `1` you did not answer `yes`; `2` a blocked policy
finding, an unknown service, an ambiguous name, or a change without `--yes`
where no prompt is possible; `3` not signed in; `4` no `stack.manage`,
`server.manage` or `stack.privileged`; `5` the stack needs
`stack.privileged` and your session is not MFA-verified; `6` no such stack or
revision; `8` `--wait` timed out; `10` a job failed or was cancelled.

**See also**: [`forge stacks rollback`](#forge-stacks-rollback),
[`forge jobs watch`](/docs/reference/cli/jobs/#forge-jobs-watch).

### forge stacks rollback

Deploys an older revision. Without `--to`, it deploys the revision before
the deployed one. It deploys all services and asks first unless you pass
`--yes`. Needs the same permissions as
[`forge stacks deploy`](#forge-stacks-deploy).

**Synopsis**

```text
forge stacks rollback [OPTIONS] <STACK>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<stack>` | stack id or name | — | yes | The stack | `docs-demo` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab` |
| `--to <TO>` | integer | the deployed revision − 1 | no, unless the deployed revision is `1` or none is deployed | Revision to go back to | `--to 2` |
| `--wait` | flag | off | no | Wait for the jobs and print them when they finish | `--wait` |
| `--wait-timeout-secs <WAIT_TIMEOUT_SECS>` | integer, seconds | `1800` | no | How long `--wait` waits. The jobs keep running after a timeout | `--wait-timeout-secs 600` |
| `--yes` | flag | off | no | Skip the confirmation prompt | `--yes` |

**Examples**

```sh
# Go back to the previous revision and wait for the result
forge stacks rollback <stack> --environment <environment> --wait --yes
# Go back to a given revision
forge stacks rollback <stack> --to 2 --yes
```

**Output**

The same as [`forge stacks deploy`](#forge-stacks-deploy): the jobs it
started, or with `--wait` the finished jobs. Shape from the server's schema;
not captured on a live system.

Exit codes: `0` success; `1` you did not answer `yes`; `2`
`no_previous_revision` (no `--to` and the deployed revision is `1` or none
is deployed), a blocked policy finding, or an ambiguous name; `3` not signed
in; `4` permission denied; `5` MFA needed for a stack with host-access
settings; `6` no such stack or revision; `8` `--wait` timed out; `10` a job
failed or was cancelled.

**See also**: [`forge stacks revisions`](#forge-stacks-revisions),
[`forge stacks deploy`](#forge-stacks-deploy).

### forge stacks service

Starts, stops, restarts or pulls the stack's services on one node. It does
not ask for confirmation. Needs `stack.manage` on the environment.

**Synopsis**

```text
forge stacks service [OPTIONS] --node <NODE> <STACK> <ACTION>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<stack>` | stack id or name | — | yes | The stack | `docs-demo` |
| `<action>` | enum: `start`, `stop`, `restart`, `pull` | — | yes | What to do with the services | `restart` |
| `--node <NODE>` | node id, hostname or display name | — | yes | The node | `--node web-01` |
| `--services <SERVICES>` | list | all services on that node | no | Services to act on | `--services web` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab` |
| `--wait` | flag | off | no | Wait up to 600 seconds for the job and print it | `--wait` |

**Examples**

```sh
# Restart every service of the stack on one node
forge stacks service <stack> restart --node <node-name> --wait
# Pull new images for one service
forge stacks service <stack> pull --node <node-name> --services <service>
```

**Output**

```text
JOB                                   KIND          STATUS
01900000-0000-7000-8000-000000000020  stack.action  succeeded
```

With `-o json`, `result` is the [job object](/docs/reference/cli/jobs/#forge-jobs-get),
or with `--wait` an array with the finished job.

Exit codes: `0` success; `2` an unknown action, a service that does not run
on this node, or an ambiguous name; `3` not signed in; `4` no
`stack.manage`; `6` no such stack or node, or the stack is not deployed on
this node; `8` `--wait` timed out; `10` the job failed or was cancelled.

**See also**: [`forge stacks logs`](#forge-stacks-logs),
[`forge stacks deploy`](#forge-stacks-deploy).

### forge stacks logs

Prints the last log lines of one service on one node. Needs `view` on the
stack's environment.

**Synopsis**

```text
forge stacks logs [OPTIONS] --node <NODE> <STACK> <SERVICE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<stack>` | stack id or name | — | yes | The stack | `docs-demo` |
| `<service>` | string | — | yes | Service name from the compose file | `hello` |
| `--node <NODE>` | node id, hostname or display name | — | yes | The node the service runs on | `--node web-01` |
| `--tail <TAIL>` | integer, lines | `200` | no | Number of lines, up to 10,000 | `--tail 50` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab` |

**Examples**

```sh
# The last 200 lines
forge stacks logs <stack> <service> --node <node-name>
# The last 20 lines, searched for errors
forge stacks logs <stack> <service> --node <node-name> --tail 20 | grep -i error
```

**Output**

The table output prints the log lines as plain text. With `-o json`:

```json
{
  "result": {
    "job_id": "01900000-0000-7000-8000-000000000021",
    "logs": ""
  },
  "schema": "forge.cli/v1"
}
```

When the node takes longer than 25 seconds, `result` is
`{ "job_id": "<job-id>", "status": "running" }` instead. Follow it with
[`forge jobs watch`](/docs/reference/cli/jobs/#forge-jobs-watch).

Exit codes: `0` success; `2` the service does not run on this node, or an
ambiguous name; `3` not signed in; `4` no `view`; `6` no such stack or node,
or the stack is not deployed on this node; `9` the server is unreachable, or
the node failed to read the logs (`node_error`).

**See also**: [`forge stacks service`](#forge-stacks-service).

### forge stacks refresh

Reads the service state from every node of the stack again. It starts one
`stack.action` job per node and prints them. Needs `view` on the stack's
environment.

**Synopsis**

```text
forge stacks refresh [OPTIONS] <STACK>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<stack>` | stack id or name | — | yes | The stack | `docs-demo` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab` |

**Examples**

```sh
# Refresh, then show the new state
forge stacks refresh <stack>
forge stacks show <stack>
```

**Output**

The jobs it started, as a key/value list. A stack that runs on no node prints
`[]`.

```text
[
  {
    "attempts": 0,
    "context": {
      "mfa": true
    },
    "created_at": 1794590572644,
    "current_step": 0,
    "id": "01900000-0000-7000-8000-000000000022",
    "idempotency_key": null,
    "kind": "stack.action",
    "last_error": null,
    …
  }
]
```

With `-o json`, the same array is in `result`.

Exit codes: `0` success; `2` ambiguous name; `3` not signed in; `4` no
`view`; `6` no such stack.

**See also**: [`forge stacks show`](#forge-stacks-show),
[`forge jobs wait`](/docs/reference/cli/jobs/#forge-jobs-wait).

### forge stacks down

Stops and removes the stack's containers on all its nodes, or on one node
with `--node`. Files in a folder you chose with `--dir` stay. It asks first
unless you pass `--yes`. Needs `stack.manage` on the environment.

**Synopsis**

```text
forge stacks down [OPTIONS] <STACK>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<stack>` | stack id or name | — | yes | The stack | `docs-demo` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab` |
| `--node <NODE>` | node id, hostname or display name | all nodes | no | Only on this node | `--node web-01` |
| `--volumes` | flag | off | no | Also remove the stack's named volumes | `--volumes` |
| `--wait` | flag | off | no | Wait for the jobs and print them when they finish | `--wait` |
| `--wait-timeout-secs <WAIT_TIMEOUT_SECS>` | integer, seconds | `1800` | no | How long `--wait` waits. The jobs keep running after a timeout | `--wait-timeout-secs 600` |
| `--yes` | flag | off | no | Skip the confirmation prompt | `--yes` |

:::caution
`down --volumes` deletes the data in the stack's named volumes. It cannot be
undone.
:::

**Examples**

```sh
# Take the stack down everywhere and wait
forge stacks down <stack> --wait --yes
# Take it down on one node only
forge stacks down <stack> --node <node-name> --yes
```

**Output**

```text
JOB                                   KIND          STATUS
01900000-0000-7000-8000-000000000023  stack.action  succeeded
```

With `-o json`, `result` is an array of
[job objects](/docs/reference/cli/jobs/#forge-jobs-get).

Exit codes: `0` success; `1` you did not answer `yes`; `2` an ambiguous
name, or a change without `--yes` where no prompt is possible; `3` not
signed in; `4` no `stack.manage`; `6` no such stack or node; `7` the stack is
not deployed (on that node); `8` `--wait` timed out; `10` a job failed or was
cancelled.

**See also**: [`forge stacks delete`](#forge-stacks-delete).

### forge stacks delete

Deletes a stack and all its revisions. The stack must be down on every node
first. It asks first unless you pass `--yes`. Needs `stack.manage` on the
environment.

**Synopsis**

```text
forge stacks delete [OPTIONS] <STACK>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<stack>` | stack id or name | — | yes | The stack | `docs-demo` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab` |
| `--yes` | flag | off | no | Skip the confirmation prompt | `--yes` |

**Examples**

```sh
# Take the stack down, then delete it
forge stacks down <stack> --wait --yes
forge stacks delete <stack> --yes
```

**Output**

Nothing on success. With `-o json`, `result` is `null`.

Exit codes: `0` success; `1` you did not answer `yes`; `2` an ambiguous
name, or a change without `--yes` where no prompt is possible; `3` not
signed in; `4` no `stack.manage`; `6` no such stack; `7` the stack still runs
on a node.

**See also**: [`forge stacks down`](#forge-stacks-down).

## Stack documents

These commands read a [`forge.stack.yaml`](/docs/reference/stack-yaml/). Paths
in the document (`spec.compose.file`, `spec.compose.envFile`) are relative to
the document's folder; the CLI reads them and sends their text, the server
never reads files. For a walkthrough, see
[Deploy a stack from YAML](/docs/guides/deploy-stack-with-yaml/).

### forge stacks validate

Checks a stack document offline: every field (errors name the field path),
the override for the environment, that the compose and `.env` files exist,
and that several `placement.nodes` come with per-service placement. Policy
checks of the compose file need the server; run
[`forge stacks plan`](#forge-stacks-plan) for those.

**Synopsis**

```text
forge stacks validate [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `-f`, `--file <FILE>` | path, or `-` for stdin | `forge.stack.yaml` | no | The stack document | `-f deploy/forge.stack.yaml` |
| `--environment <SLUG>` | environment slug | `metadata.environment` | no | Read the document for this environment; its `overrides` entry is merged in | `--environment staging` |

**Example**

```console
$ forge stacks validate -f forge.stack.yaml -o json
{"valid":true,"stack":"orders","environment":"prod","order":"node-by-node","health_timeout_s":300,"placement":{"cluster":"c1"},"mesh":true,"env_file":true}
```

**Exit status**: `0` valid; `1` the document is refused (the message names
the field, for example `spec.rollout.healthTimeout: between 10s and 1h`); `2`
the document file cannot be read.

### forge stacks plan

Sends the document to `POST /api/v1/stacks/plan` and prints what `apply`
would do: create or update, the next revision, the nodes in rollout order
with their services, nodes the new revision leaves, the cluster network per
node and every finding. Saves nothing.

**Synopsis**

```text
forge stacks plan [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `-f`, `--file <FILE>` | path, or `-` for stdin | `forge.stack.yaml` | no | The stack document | `-f forge.stack.yaml` |
| `--environment <SLUG>` | environment slug | `metadata.environment` | no | Plan for this environment instead | `--environment staging` |

**Example**

```console
$ forge stacks plan -f forge.stack.yaml
Stack        orders in prod (update, revision 7)
Rollout      node-by-node, health timeout 5m
Nodes
  lab-b                api, worker
  lab-c                api
Network      cluster network on lab-b; not in sync: lab-c
info         lab-c: cluster network not applied yet; services will join it on the next deploy after it is in sync
```

**Exit status**: `0` the plan can be applied; `1` the document or the plan
is refused (blocked findings, unknown node or cluster); `4` no permission.

### forge stacks apply

Plans the document, prints the plan, asks for confirmation, then calls
`POST /api/v1/stacks/apply`: the stack is created or updated (a new revision
noted `apply <first 12 hex digits of the document's SHA-256>` when the
compose text changed) and a `stack.rollout` job deploys it node by node or
all at once. With `--wait` it follows the job and, when the stack is live,
prints its cluster names from `GET /api/v1/names?stack=<id>`.

**Synopsis**

```text
forge stacks apply [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `-f`, `--file <FILE>` | path, or `-` for stdin | `forge.stack.yaml` | no | The stack document | `-f forge.stack.yaml` |
| `--environment <SLUG>` | environment slug | `metadata.environment` | no | Apply to this environment instead; its `overrides` entry is merged in | `--environment prod` |
| `--wait` | flag | off | no | Follow the rollout until it ends | `--wait` |
| `--timeout <MINUTES>` | integer | `60` | no | Longest wait with `--wait` | `--timeout 30` |
| `--yes` | flag | off | no | Start without asking | `--yes` |

The global `--non-interactive` flag also starts without asking.

**Example**

```console
$ forge stacks apply -f forge.stack.yaml --wait --yes
…
stack orders revision 7 is live on 2 nodes
names:
  api.orders.prod.c1.local      10.64.10.2, 10.64.12.4
  worker.orders.prod.c1.local   10.64.10.3
```

**Exit status**: `0` the stack is live (or the rollout started, without
`--wait`); `1` the document or the plan is refused; `2` a node failed and
later nodes were not deployed; `7` a rollout of this stack is already
running; `8` `--wait` timed out (the rollout keeps running).

**See also**: [`forge stacks plan`](#forge-stacks-plan),
[Stack YAML](/docs/reference/stack-yaml/).

## Templates

Ready compose apps with pinned images (see
[Docker resources](/docs/guides/docker-resources/)). Empty secret
variables are generated and stored sealed in the stack's `.env`.

### forge stacks templates

Compose app templates and their variables

**Synopsis**

```text
forge stacks templates [OPTIONS]
```

### forge stacks from-template

Create a compose stack from a template

**Synopsis**

```text
forge stacks from-template [OPTIONS] --environment <ENVIRONMENT> --node <NODE> <TEMPLATE> <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<template>` | string | - | yes | Template id (see `forge stacks templates`) | - |
| `<name>` | string | - | yes | Name of the new stack | - |
| `--environment` | string | - | no | Environment (id or name) | - |
| `--node` | string | - | no | Node it runs on (id, host name or name) | - |
| `--set` | flag | - | no | <KEY=VALUE> A variable (`KEY=VALUE`, repeat). Empty secrets are generated | - |

