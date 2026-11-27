---
title: forge services
description: Create, change, place, deploy, inspect and delete services, and attach buckets to them, from the command line.
sidebar:
  order: 9
---

`forge services` works with services: what runs in an environment, as a
systemd unit or a container. It uses the same
[service endpoints](/docs/reference/rest-api/#services) as the console.
For a walkthrough, see [Services](/docs/guides/services/). For the model,
see [Projects, environments and services](/docs/concepts/projects-environments-services/).
Every command takes the [global flags](/docs/reference/cli/#global-flags).

`<service>` is a service id or name. A name must match exactly one
service you can see; when it matches several, the command exits with `2`
(`ambiguous`) and lists the ids. Pass `--environment` (id or name) or the id.
`--node` takes a node id, hostname or display name.

:::note
Services that a Compose stack registered belong to the stack. `forge services
action`, `deploy` and `delete` refuse them with exit `7` (`conflict`). Use
[`forge stacks service`](/docs/reference/cli/stacks/#forge-stacks-service) and the
other [`forge stacks`](/docs/reference/cli/stacks/) commands for them. `list` and
`show` include them.
:::

## Permissions

| Commands | Cedar action | Scope | MFA |
| --- | --- | --- | --- |
| `list`, `show`, `action … status`, `logs`, `storage list` | `view` | the service's environment | no |
| `create`, `update`, `place`, `remove`, `action … start`, `stop`, `restart`, `deploy`, `delete`, `storage attach`, `storage detach` | `deploy` | the service's environment | no |
| `place` | `server.manage` | each node in `--node` | no |
| `files …` | `file.read`, `file.write` (changes) | the app root on each node | changes: yes |
| `deploy` when a node lacks a runtime the definition lists in `dependencies`, or Docker Engine for a container | `package.manage` | each node that lacks it | yes (step-up) |
| `deploy` with a container volume outside the app's folder and the node's approved file roots | `stack.privileged` | the service's environment | yes |
| `storage attach` of a bucket whose store does not belong to the service's environment, or that is attached to a service of another environment | `storage.manage` | the store | yes |

`deploy` is in the `platform_admin`, `workspace_admin`, `operator` and
`developer` roles. `package.manage` and `storage.manage` are in
`platform_admin`, `workspace_admin` and `operator`; `stack.privileged` is in
`platform_admin` and `workspace_admin`. See the
[Cedar schema](/docs/reference/cedar-schema/).

## Commands

| Command | What it does |
| --- | --- |
| [`forge services list`](#forge-services-list) | Lists the services and services you can see |
| [`forge services show`](#forge-services-show) | Shows one service with its instances |
| [`forge services health`](#forge-services-health) | Shows the service's health checks, one line each |
| [`forge services action`](#forge-services-action) | Runs `start`, `stop`, `restart` or `status` on one instance |
| [`forge services deploy`](#forge-services-deploy) | Deploys the service on its nodes, installing missing runtimes and Docker Engine first |
| [`forge services delete`](#forge-services-delete) | Deletes a service that runs on no node |
| [`forge services create`](#forge-services-create) | Creates a service from flags, from a JSON definition, or both |
| [`forge services update`](#forge-services-update) | Changes single fields of a service's definition, or replaces it from a file |
| [`forge services place`](#forge-services-place) | Sets the nodes the service is placed on, each as active or standby |
| [`forge services storage list`](#forge-services-storage-list) | Lists the buckets attached to a service |
| [`forge services storage attach`](#forge-services-storage-attach) | Attaches a bucket of a managed store to a service |
| [`forge services storage detach`](#forge-services-storage-detach) | Detaches a bucket and revokes its access key |
| [`forge services logs`](#forge-services-logs) | Prints or follows one instance's log lines |
| [`forge services remove`](#forge-services-remove) | Takes the service off one node |
| [`forge services files ls`](#forge-services-files-ls) | Lists a folder of the app root on its nodes |
| [`forge services files upload`](#forge-services-files-upload) | Uploads a file to the app root on every node |
| [`forge services files extract`](#forge-services-files-extract) | Unpacks an archive in the app root on every node |
| [`forge services files mkdir`](#forge-services-files-mkdir) | Creates a folder in the app root on every node |

### forge services list

Lists the services and services you can see, with how many of their
instances run. Needs `view` on each service's environment; others are left
out.

**Synopsis**

```text
forge services list [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--environment <ENVIRONMENT>` | environment id or name | all environments | no | Only this environment | `--environment lab` |
| `--cluster <CLUSTER>` | cluster id, name or abbreviation (case-insensitive) | all clusters | no | Only services in this cluster's environments. Ignored when you pass `--environment` | `--cluster <cluster-name>` |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

**Examples**

```sh
# Every service you can see
forge services list
# Services in one environment
forge services list --environment <environment>
# Services in one cluster, by name or abbreviation
forge services list --cluster <cluster-name>
```

**Output**

The capture comes from a lab with no services:

```text
ID  NAME  RUNTIME  ENVIRONMENT  RUNNING
```

`RUNNING` is `<running>/<instances>`, such as `1/2`: the instances whose
`state` is `active` or `running`, out of all instances. `state` is what the
node last reported for the unit or container. Forge's own `status`
(`pending`, `deployed`, `failed`, …) does not count, so an instance that is
`deployed` and has not reported a state yet is not counted as running.
`RUNTIME` is `systemd`, `container`, or the stack runtime for services a
Compose stack registered.

```json
{
  "result": [],
  "schema": "forge.cli/v1"
}
```

With services, each `result` item has the fields shown in
[`forge services show`](#forge-services-show) plus `instances_summary` (the
`RUNNING` value).

Exit codes: `0` success; `2` the environment or cluster name is ambiguous; `3`
not signed in; `6` no environment or cluster with that name.

**See also**: [`forge services show`](#forge-services-show),
[Services](/docs/guides/services/).

### forge services show

Shows one service as a readable summary: name, runtime and environment;
for a systemd service the unit
(`forge-app-<environment>-<service>.service`), the folder
(`/srv/forge/apps/<environment>/<service>`) and the account it runs as
(`<environment>-<service>`); the spec without empty fields (container-only fields
are left out for systemd services); the environment, with secret values
hidden; and one line per instance with the node's name, role, status and
state. `forge services get` is an alias. Needs `view` on the service's
environment.

**Synopsis**

```text
forge services show [OPTIONS] <SERVICE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | service id or name | — | yes | The service | `billing-api` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab` |

**Examples**

```sh
# One service
forge services show <service>
# The state of each instance
forge -o json services show <service> | jq '.result.instances[] | {server_id, status, state}'
```

**Output**

```text
$ forge services show xcweb
name        xcweb
id          <service-id>
runtime     systemd
environment   lab-test
unit        forge-app-lab-test-xcweb.service
folder      /srv/forge/apps/lab-test/xcweb
user        lab-test-xcweb
spec:
  command      python3 -m http.server 8080
  listen_port  8080
  restart      yes
env         PORT=8080, TOKEN=(secret)
instances:
  NODE   ROLE    STATUS    STATE   ERROR
  lab-b  active  deployed  active  -
  lab-c  active  deployed  active  -
updated_at  2026-11-14 18:20:25
```

The account is the readable name Forge tries first; a node gives the app a
hashed name only when that one is too long or taken. With `-o json`, the full
record:

```json
{
  "result": {
    "id": "<service-id>",
    "workspace_id": "<workspace-id>",
    "environment_id": "<environment-id>",
    "name": "<service>",
    "runtime": "systemd",
    "spec": { … },
    "env": [ … ],
    "created_by": { "id": "<user-id>", "type": "user" },
    "created_at": 1794590549602,
    "updated_at": 1794590549602,
    "instances": [
      {
        "service_id": "<service-id>",
        "server_id": "<node-id>",
        "role": "active",
        "status": "deployed",
        "state": "active",
        "last_job_id": "<job-id>",
        "last_error": null,
        "updated_at": 1794590549602
      }
    ]
  },
  "schema": "forge.cli/v1"
}
```

Shape from the server's schema; not captured on a live system. `role` is
`active` or `standby`. `status` is Forge's deploy state (`pending`,
`deploying`, `deployed`, `failed`, …); `state` is the last state the node
reported (`active`, `inactive`, `failed`, …).

Exit codes: `0` success; `2` ambiguous name; `3` not signed in; `4` no
`view`; `6` no such service.

**See also**: [`forge services action`](#forge-services-action).

### forge services health

Shows the service's startup, readiness and liveness checks, one line per
check, with a plain-language summary of what each check does and when it
changes state. Needs `health.view` on the service. Checks are edited in
the console (form or YAML); see [Health checks](/docs/concepts/health-checks/).

**Synopsis**

```text
forge services health <SERVICE> [--environment <ENVIRONMENT>]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | service id or name | none | yes | The service | `api` |
| `--environment` | environment id or name | none | no | Needed when the name exists in several environments | `staging` |

**Examples**

```sh
# The checks of one service
forge services health api --environment staging

# The full policy as JSON, e.g. to edit and send back with `forge api call`
forge -o json services health api | jq '.result.policy'
```

**Output**

```text
PURPOSE    NAME     ENABLED  SUMMARY
startup    init     yes      Check GET http://the workload:8080/health/startup every 5 s (timeout 2 s). Give up after 60 failures (the startup budget); readiness and liveness wait until it succeeds.
readiness  api      yes      Check GET http://the workload:8080/health/ready every 10 s (timeout 2 s). Mark unready after 3 consecutive failures; ready again after 2 successes.
liveness   tcp      yes      Open a TCP connection to the workload port 8080 every 15 s (timeout 2 s). Report not live after 3 consecutive failures. Do not restart.
```

With `-o json`, the API response: `policy`, `summaries`, `legacy_healthcheck`
and `updated_at`. See
[REST API: Service health checks](/docs/reference/rest-api/#service-health-checks).

**Errors**

| Error | Cause | Fix |
| --- | --- | --- |
| `no service named ... that you can see` | Wrong name, or no access | Check the name and your roles |
| `403 Forbidden` | You lack `health.view` | Ask for a role on the environment |

### forge services action

Runs an action on the service's instance on one node. `start`, `stop`
and `restart` ask first unless you pass `--yes` and need `deploy` on the
environment. `status` does not ask and needs `view`.

**Synopsis**

```text
forge services action [OPTIONS] --node <NODE> <SERVICE> <ACTION>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | service id or name | — | yes | The service | `billing-api` |
| `<action>` | enum: `start`, `stop`, `restart`, `status`, `prepare` | — | yes | What to do. `prepare` creates the app folder, its user and the app root on the node before the first deploy, so you can upload code first | `restart` |
| `--node <NODE>` | node id, hostname or display name | — | yes | The node of the instance | `--node web-01` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab` |
| `--yes` | flag | off | no | Skip the confirmation prompt for `start`, `stop` and `restart` | `--yes` |

**Examples**

```sh
# Ask the node for the instance's state
forge services action <service> status --node <node-name>
# Restart the instance without a prompt
forge services action <service> restart --node <node-name> --yes
```

**Output**

The server waits up to 25 seconds for the node. With `-o json`, `result` is
one of:

```json
{ "job_id": "<job-id>", "status": "succeeded", "result": { … } }
```

```json
{ "job_id": "<job-id>", "status": "running" }
```

The table output prints the same object as a key/value list. Follow a
`running` job with [`forge jobs watch`](/docs/reference/cli/jobs/#forge-jobs-watch).

When the action fails on the node, the command exits `10` with the error
code `action_failed`. The table output prints the result first, with
`"status": "failed"` and the node's `error`, then the error message on
stderr. With `-o json`, stdout holds only the
[error document](/docs/reference/cli/#exit-codes). Shape from the server's schema;
not captured on a live system.

Exit codes: `0` the action succeeded or still runs; `1` you did not answer
`yes`; `2` an unknown action, an ambiguous name, or a change without `--yes`
where no prompt is possible; `3` not signed in; `4` no `deploy` (or `view`
for `status`); `6` no such service or node, or the service is not
placed on this node; `7` the service belongs to a Compose stack; `10`
the action failed on the node (`action_failed`).

**See also**: [`forge services show`](#forge-services-show),
[`forge stacks service`](/docs/reference/cli/stacks/#forge-stacks-service).

### forge services deploy

Deploys the service on the nodes it is placed on. It asks first unless
you pass `--yes`. Needs `deploy` on the environment.

Before the deploy, the server installs what a node lacks:

- each runtime the definition lists in `dependencies` that is not installed
  on the node in that version, or being installed;
- for a container, Docker Engine (the App Store package `docker`, version
  `stable`) on a Linux node that has none.

These are App Store installs. They need `package.manage` on the node with an
MFA-verified session; without it the command exits with `4` or `5` and
nothing starts. The deploy job of a node waits for that node's installs, up
to 45 minutes. It fails when a runtime install failed, and with `Docker
Engine failed to install on this node` when the Docker Engine install it
waited for failed.

Forge installs Docker Engine only on a node that is known to lack it:

| The node | What the deploy does |
| --- | --- |
| Has a Docker Engine installation from the App Store (installed, or being installed) | No install. The deploy job waits for a running install |
| Has no such installation, and answers that Docker is not available | Installs Docker Engine first |
| Has no such installation, and its Docker Engine answers (installed outside Forge) | No install |
| Is offline, or does not answer within 15 seconds | No install. The deploy job reports the node's own error |

:::note
Deploying to one node with
[`forge services action <service> deploy`](#forge-services-action) installs nothing. Use
`forge services deploy` for the first deploy on a node.
:::

**Synopsis**

```text
forge services deploy [OPTIONS] <SERVICE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | service id or name | — | yes | The service | `billing-api` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab` |
| `--wait` | flag | off | no | Wait for the jobs and print them when they finish | `--wait` |
| `--wait-timeout-secs <WAIT_TIMEOUT_SECS>` | integer, seconds | `1800` | no | How long `--wait` waits. The jobs keep running after a timeout | `--wait-timeout-secs 600` |
| `--yes` | flag | off | no | Skip the confirmation prompt | `--yes` |

**Examples**

```sh
# Deploy and wait for the result
forge services deploy <service> --wait --yes
# CI: never prompt, fail on a failed job
forge services deploy <service> --environment <environment> --wait --yes --non-interactive
```

**Output**

The jobs it started: the `package.action` install jobs first (Docker Engine,
then one per missing runtime and node), then one `app.action` job per node. When there are
install jobs, the table output first prints one line on stderr that names
the packages:

```text
Installing what the nodes lack first (docker): 1 install job(s). The deploy jobs wait for them.
JOB                     KIND            STATUS
<job-id>                package.action  queued
<job-id>                app.action      queued
```

With `--wait`, the same columns for the finished jobs. With `-o json`,
`result` is an array of [job objects](/docs/reference/cli/jobs/#forge-jobs-get),
install jobs first, and the stderr line is not printed:

```json
{
  "result": [
    { "id": "<job-id>", "kind": "package.action", "status": "queued", "payload": { "package": "python", "action": "install", … }, … },
    { "id": "<job-id>", "kind": "app.action", "status": "queued", … }
  ],
  "schema": "forge.cli/v1"
}
```

Shape from the server's schema; not captured on a live system.

Exit codes: `0` success; `1` you did not answer `yes`; `2` an invalid
service spec, an ambiguous name, or a change without `--yes` where no
prompt is possible; `3` not signed in; `4` no `deploy` or `package.manage`;
`5` a runtime install needs an MFA-verified session; `6` no such
service; `7` the service is placed on no node, or belongs to a
Compose stack; `8` `--wait` timed out; `10` a job failed or was cancelled.

**See also**: [`forge jobs watch`](/docs/reference/cli/jobs/#forge-jobs-watch),
[Services](/docs/guides/services/).

### forge services delete

Deletes a service. Every instance must be removed from its node first;
instances that were placed but never deployed (`pending`) do not count. It
asks first unless you pass `--yes`. Needs `deploy` on the environment.

**Synopsis**

```text
forge services delete [OPTIONS] <SERVICE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | service id or name | — | yes | The service | `billing-api` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab` |
| `--yes` | flag | off | no | Skip the confirmation prompt | `--yes` |

**Examples**

```sh
# Delete a service without a prompt
forge services delete <service> --environment <environment> --yes
```

**Output**

Nothing on success. With `-o json`, `result` is `null`. Shape from the
server's schema; not captured on a live system.

Exit codes: `0` success; `1` you did not answer `yes`; `2` an ambiguous
name, or a change without `--yes` where no prompt is possible; `3` not
signed in; `4` no `deploy`; `6` no such service; `7` the service
still runs on a node, or belongs to a Compose stack.

**See also**: [`forge services show`](#forge-services-show).

### forge services create

Creates a service (a service) in an environment. Give the definition
as flags, as a JSON file, or both: the file is the base and each flag
changes one of its fields. The CLI checks the body against the server's
schema before sending it. The command does not ask first. Needs `deploy` on
the environment.

Creating does not run anything. Next, pick the nodes with
[`forge services place`](#forge-services-place) and run
[`forge services deploy`](#forge-services-deploy).

**Synopsis**

```text
forge services create [OPTIONS]
```

**Arguments and flags**

Without `--file`, `--environment`, `--name` and `--runtime` are required. With
`--file`, each of them is required only when the file lacks that field. A
missing one exits with `2` and names the flag.

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--file <FILE>` | path, `@<path>` or `@-` | none | no | The [definition](#service-definition) as JSON; `@-` reads stdin. The flags below change its fields | `--file app.json` |
| `--environment <ENVIRONMENT>` | environment id or name | the file's `environment_id` | yes, without `--file` | The environment the service is created in | `--environment shop-prod` |
| `--name <NAME>` | string | the file's `name` | yes, without `--file` | The service's name: lower-case letters, digits and dashes, starting with a letter and not ending with a dash, at most 40 characters. Unique in the environment | `--name web` |
| `--runtime <RUNTIME>` | enum: `systemd`, `container` | the file's `runtime` | yes, without `--file` | What runs: `systemd` is a native systemd service from `--command`, `container` runs `--image` | `--runtime container` |
| `--image <IMAGE>` | string | none | for a container | The container image | `--image nginx:1.27` |
| `--command <COMMAND>` | string | none | for a systemd service | The command to run. For a container it replaces the image's command | `--command "python3 -m http.server 8080"` |
| `--port <PORT>` | `[IP:]HOST:CONTAINER[/tcp\|udp]`; repeatable | none | no | A container port mapping. Given at all, the flags replace the file's whole `ports` list | `--port 8080:80` |
| `--env <KEY=VALUE>` | `KEY=VALUE`; repeatable | none | no | A variable. `KEY` is letters, digits and `_` and does not start with a digit. The value may be empty and may contain `=`. Sets or replaces that variable and keeps the file's others | `--env MODE=prod` |
| `--secret-env <KEY[=@FILE]>` | `KEY` or `KEY=@<path>`; repeatable | none | no | A secret variable. See [Secret variables](#secret-variables) | `--secret-env API_TOKEN` |
| `--listen-port <LISTEN_PORT>` | integer, 1 to 65535 | none | no | The TCP port a native service listens on. Forge's default health check connects to it | `--listen-port 8080` |
| `--volume <VOLUME>` | `HOST_PATH:CONTAINER_PATH[:ro]` or `VOLUME_NAME:CONTAINER_PATH[:ro]`; repeatable | none | no | A container volume. Given at all, the flags replace the file's whole `volumes` list | `--volume /srv/forge/apps/shop-prod/web/html:/usr/share/nginx/html:ro` |
| `--restart <BOOL>` | `true` or `false` | the server's default, `true` | no | Restart it when it stops. Sets the restart policy to `unless-stopped` (`true`) or `no` (`false`) | `--restart false` |
| `--restart-policy <POLICY>` | enum: `no`, `always`, `unless-stopped`, `on-failure` | none | no | When the service is restarted. Overrides `--restart` | `--restart-policy on-failure` |
| `--restart-max-attempts <N>` | integer, 0 to 1000 | none | no | With `on-failure` only: stop restarting after N restarts. `0` means no limit. Under any other policy the command exits with `2`. A stored count is cleared when you switch to another policy | `--restart-max-attempts 5` |
| `--cap-add <CAP>` | capability name; repeatable | none | no | A Linux capability to add (container only). Deploying then needs `stack.privileged`. Given at all, the flags replace the whole list | `--cap-add NET_ADMIN` |
| `--cap-drop <CAP>` | capability name or `ALL`; repeatable | none | no | A Linux capability to drop (container only) | `--cap-drop ALL` |
| `--dns <IP>` | IP address; repeatable | none | no | A DNS server for the container, at most 8 | `--dns 192.0.2.53` |
| `--tmpfs <PATH[:SIZE_MB]>` | container path with an optional size; repeatable | none | no | A tmpfs mount. Without a size, Docker's default applies | `--tmpfs /tmp:64` |
| `--init <BOOL>` | `true` or `false` | none | no | Run Docker's init as PID 1, which reaps zombie processes and forwards signals; `--init` alone means `true` | `--init` |
| `--network <NAME>` | Docker network name; repeatable | none | no | An existing Docker network on the node to connect the container to as well | `--network backend` |

**Examples**

```sh
# A container service
forge services create --environment lab --name web --runtime container --image nginx:1.27 --port 8080:80
# A systemd service with an environment variable
forge services create --environment lab --name api --runtime systemd --command "python3 -m http.server 8080" --listen-port 8080 --env PORT=8080
# From a JSON definition
forge services create --environment lab --file service.json
```

Fields without a flag need `--file`: `working_dir`, `env_files`,
`dependencies`, `cpu_millis`, `memory_mb`, `entrypoint`, `user`, `hostname`,
`labels`, `extra_hosts`, `healthcheck`, `stop_timeout_secs`, `read_only`,
`log_max_mb`, `ulimits`, `sysctls` and `devices`. See
[Service definition](#service-definition).

#### Secret variables

`--secret-env` never takes the value from the command line, so the value
stays out of your shell history and the process list.

| Form | Where the value comes from |
| --- | --- |
| `--secret-env KEY` (one such key) | All of stdin, without one trailing line break |
| `--secret-env KEY` (several such keys) | One `KEY=VALUE` line per key on stdin |
| `--secret-env KEY=@<path>` | The file, without one trailing line break |
| `--secret-env KEY=@-` | Stdin, as for `KEY` |

A value may not be empty and may not contain a line break.

:::caution
`--secret-env KEY=value` is refused with exit `2`
(`secret_on_command_line`); the value is not sent anywhere. `--file @-` and
a `--secret-env KEY` that reads stdin cannot be combined (exit `2`): give
the secret as `KEY=@<path>`. The server seals a secret value and never
returns it; `forge services show` prints `KEY=(secret)`.
:::

**Examples**

```sh
# A container with a published port and a variable
forge services create --environment shop-prod --name web --runtime container \
  --image nginx:1.27 --port 8080:80 --env MODE=prod

# A native service with the port it listens on
forge services create --environment shop-prod --name api --runtime systemd \
  --command "python3 -m http.server 8080" --listen-port 8080

# A secret from stdin: all of stdin is the value
printf '%s' "$TOKEN" | forge services create --environment shop-prod --name worker \
  --runtime systemd --command "/srv/forge/apps/shop-prod/worker/run" --secret-env API_TOKEN

# A secret from a file
forge services create --environment shop-prod --name worker --runtime systemd \
  --command "/srv/forge/apps/shop-prod/worker/run" --secret-env API_TOKEN=@token.txt

# Two secrets from stdin, one KEY=VALUE line each
printf 'DB_PASSWORD=%s\nAPI_TOKEN=%s\n' "$DB_PASSWORD" "$TOKEN" | forge services create \
  --environment shop-prod --name worker --runtime systemd \
  --command "/srv/forge/apps/shop-prod/worker/run" --secret-env DB_PASSWORD --secret-env API_TOKEN

# From a file
forge services create --file app.json

# From a file, in another environment and with another image
forge services create --file app.json --environment shop-uat --image nginx:1.28
```

**Output**

The new service record. Shape from the server's schema; not captured on
a live system. The table output prints the same record as a key/value list.

```json
{
  "result": {
    "id": "<service-id>",
    "workspace_id": "<workspace-id>",
    "environment_id": "<environment-id>",
    "name": "web",
    "runtime": "container",
    "spec": {
      "environment": "shop-prod",
      "image": "nginx:1.27",
      "ports": ["8080:80"],
      "restart": true,
      …
    },
    "env": [
      { "key": "MODE", "value": "prod", "secret": false },
      { "key": "API_TOKEN", "value": null, "secret": true }
    ],
    "created_by": { "id": "<user-id>", "type": "user" },
    "created_at": 1794590549602,
    "updated_at": 1794590549602
  },
  "schema": "forge.cli/v1"
}
```

Exit codes: `0` success; `2` a required flag is missing, a bad `--env` or
`--secret-env`, a secret value on the command line
(`secret_on_command_line`), the file is not valid JSON, the body does not
match the schema (`invalid_body`), or the server refuses the definition
(such as a container without an image); `3` not signed in; `4` no `deploy`;
`6` no such environment; `7` the name is taken in the environment.

**See also**: [`forge services place`](#forge-services-place),
[`forge services deploy`](#forge-services-deploy),
[`forge environments create`](/docs/reference/cli/tenants-environments/#forge-environments-create),
[Services](/docs/guides/services/).

### forge services update

Changes a service's definition. Running instances keep the old
definition until you run [`forge services deploy`](#forge-services-deploy). Asks
first unless you pass `--yes`. Needs `deploy` on the environment.

| You pass | What changes |
| --- | --- |
| Flags only | The CLI reads the stored definition and changes only the fields the flags name. Everything else stays, including stored secrets and the health policy |
| `--file` | The file replaces the whole definition. A field the file leaves out goes back to its default; a secret variable listed without a value keeps its stored value |
| `--file` and flags | The file replaces the definition, then the flags change its fields |

The CLI sends back the version of the service it read. If someone else
saved the service in between, the server refuses the update with `409`
(`this service changed since you opened it`) and nothing changes. Run the
command again to apply your change on top of theirs.

Pass at least one of `--file`, a definition flag or `--unset-env`; otherwise
the command exits with `2`. The name and the environment of a service
cannot be changed.

**Synopsis**

```text
forge services update [OPTIONS] <SERVICE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | service id or name | — | yes | The service | `web` |
| `--file <FILE>` | path, `@<path>` or `@-` | none | no | A full [definition](#service-definition) as JSON; `@-` reads stdin. It replaces the stored one | `--file app.json` |
| `--runtime <RUNTIME>` | enum: `systemd`, `container` | keep | no | What runs | `--runtime container` |
| `--image <IMAGE>` | string | keep | no | The container image | `--image nginx:1.28` |
| `--command <COMMAND>` | string | keep | no | The command to run. For a container it replaces the image's command | `--command "node server.js"` |
| `--port <PORT>` | `[IP:]HOST:CONTAINER[/tcp\|udp]`; repeatable | keep | no | Container port mappings. Given at all, the flags replace the whole list | `--port 8080:80 --port 8443:443` |
| `--env <KEY=VALUE>` | `KEY=VALUE`; repeatable | keep | no | Sets or replaces one variable and keeps the others | `--env MODE=uat` |
| `--secret-env <KEY[=@FILE]>` | `KEY` or `KEY=@<path>`; repeatable | keep | no | Sets or replaces one secret variable; the value comes from stdin or a file, never from the command line. See [Secret variables](#secret-variables) | `--secret-env API_TOKEN=@token.txt` |
| `--unset-env <KEY>` | variable name; repeatable | none | no | Removes one variable. Exits with `2` when the service has no such variable | `--unset-env OLD_FLAG` |
| `--listen-port <LISTEN_PORT>` | integer, 1 to 65535 | keep | no | The TCP port a native service listens on | `--listen-port 8080` |
| `--volume <VOLUME>` | `HOST_PATH:CONTAINER_PATH[:ro]` or `VOLUME_NAME:CONTAINER_PATH[:ro]`; repeatable | keep | no | Container volumes. Given at all, the flags replace the whole list | `--volume /srv/data:/data` |
| `--restart <BOOL>` | `true` or `false` | keep | no | Restart it when it stops. Replaces a stored restart policy with `unless-stopped` (`true`) or `no` (`false`) | `--restart false` |
| `--restart-policy <POLICY>` | enum: `no`, `always`, `unless-stopped`, `on-failure` | keep | no | When the service is restarted. Overrides `--restart` | `--restart-policy on-failure` |
| `--restart-max-attempts <N>` | integer, 0 to 1000 | keep | no | With `on-failure` only: stop restarting after N restarts. `0` means no limit. Under any other policy the command exits with `2`. A stored count is cleared when you switch to another policy | `--restart-max-attempts 5` |
| `--cap-add <CAP>` | capability name; repeatable | keep | no | A Linux capability to add (container only). Deploying then needs `stack.privileged`. Given at all, the flags replace the whole list | `--cap-add NET_ADMIN` |
| `--cap-drop <CAP>` | capability name or `ALL`; repeatable | keep | no | A Linux capability to drop (container only) | `--cap-drop ALL` |
| `--dns <IP>` | IP address; repeatable | keep | no | A DNS server for the container, at most 8 | `--dns 192.0.2.53` |
| `--tmpfs <PATH[:SIZE_MB]>` | container path with an optional size; repeatable | keep | no | A tmpfs mount. Without a size, Docker's default applies | `--tmpfs /tmp:64` |
| `--init <BOOL>` | `true` or `false` | keep | no | Run Docker's init as PID 1, which reaps zombie processes and forwards signals; `--init` alone means `true` | `--init` |
| `--network <NAME>` | Docker network name; repeatable | keep | no | An existing Docker network on the node to connect the container to as well | `--network backend` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only. It does not move the service | `--environment shop-prod` |
| `--yes` | flag | off | no | Skip the confirmation prompt | `--yes` |

:::caution
`--secret-env KEY=value` is refused with exit `2`
(`secret_on_command_line`). `--file @-` and a `--secret-env KEY` that reads
stdin cannot be combined.
:::

**Examples**

```sh
# A new image; ports, volumes and variables stay
forge services update web --image nginx:1.28 --yes && forge services deploy web --wait --yes

# Change one variable and remove another
forge services update web --env MODE=uat --unset-env OLD_FLAG --yes

# Replace the port list
forge services update web --port 8080:80 --port 8443:443 --yes

# Rotate a secret from stdin
printf '%s' "$TOKEN" | forge services update web --secret-env API_TOKEN --yes

# Replace the whole definition from a file
forge services update web --file app.json --yes
```

**Output**

The updated service record, in the shape shown under
[`forge services create`](#forge-services-create): a key/value list, or the record
under `result` with `-o json`.

Exit codes: `0` success; `1` you did not answer `yes`; `2` nothing to change
(no `--file`, definition flag or `--unset-env`), `--unset-env` names a
variable the service does not have, a bad `--env` or `--secret-env`, a
secret value on the command line (`secret_on_command_line`), an invalid file
or definition (`invalid_body`), an ambiguous name, or no `--yes` where no
prompt is possible; `3` not signed in; `4` no `deploy`; `6` no such
service; `7` the service belongs to a Compose stack.

**See also**: [`forge services deploy`](#forge-services-deploy),
[`forge services show`](#forge-services-show).

### forge services place

Sets the nodes the service is placed on, each as an active or a standby
instance. The list replaces the current placement
(`PUT /api/v1/services/{id}/instances`): a node left out is dropped when
nothing was deployed there yet; a deployed instance must be taken off with
[`forge services remove`](#forge-services-remove) first (exit `7`). Placing does not
deploy; run `forge services deploy` next. Asks first unless you pass `--yes`.
Needs `deploy` on the environment and `server.manage` on each node.

An ingress endpoint that targets the service sends traffic to active
instances and uses standby instances only when every active one is down.
See [Run on several nodes](/docs/guides/services/#run-on-several-nodes).

**Synopsis**

```text
forge services place [OPTIONS] --node <NODE[:ROLE]> <SERVICE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | service id or name | — | yes | The service | `web` |
| `--node <NODE[:ROLE]>` | `<node>`, `<node>:active` or `<node>:standby`; repeatable. `<node>` is a node id, host name or display name | role: `active`, or `standby` with `--standby` | yes | Every node the service should be on, with its role. Another role exits with `2` | `--node lab-b:active --node lab-c:standby` |
| `--standby` | flag | off | no | The role of nodes given without one becomes `standby` instead of `active` | `--standby` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment shop-prod` |
| `--yes` | flag | off | no | Skip the confirmation prompt | `--yes` |

**Examples**

```sh
# Two active instances
forge services place web --node lab-b --node lab-c --yes

# One active instance and one standby
forge services place web --node lab-b:active --node lab-c:standby --yes

# The same with --standby as the default role
forge services place web --node lab-b:active --node lab-c --standby --yes
```

**Output**

```text
NODE   ROLE     STATUS   STATE  ERROR
lab-b  active   pending  -      -
lab-c  standby  pending  -      -
```

With `-o json`, `result` is the list of instances, in the shape of
`instances` under [`forge services show`](#forge-services-show):

```json
{
  "result": [
    { "service_id": "<service-id>", "server_id": "<node-id>", "role": "active", "status": "pending", "state": null, "last_job_id": null, "last_error": null, "updated_at": 1794590549602 },
    { "service_id": "<service-id>", "server_id": "<node-id>", "role": "standby", "status": "pending", "state": null, "last_job_id": null, "last_error": null, "updated_at": 1794590549602 }
  ],
  "schema": "forge.cli/v1"
}
```

Exit codes: `0` success; `1` you did not answer `yes`; `2` a role other than
`active` or `standby`, an ambiguous name or no `--yes` without a terminal;
`3` not signed in; `4` no `deploy` or `server.manage`; `6` no such
service or node; `7` a deployed instance would be dropped, or the
service belongs to a Compose stack.

**See also**: [`forge services deploy`](#forge-services-deploy),
[`forge services remove`](#forge-services-remove).

### forge services storage list

Lists the buckets attached to a service and the variables each one
adds at deploy. An attachment is an access key limited to one bucket of a
managed store; see
[Buckets attached to services](/docs/concepts/object-storage/#buckets-attached-to-services). Needs `view`
on the environment.

**Synopsis**

```text
forge services storage list [OPTIONS] <SERVICE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | service id or name | — | yes | The service | `web` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment shop-prod` |

**Examples**

```sh
# The attached buckets
forge services storage list web

# The names of the variables the service gets
forge -o json services storage list web | jq -r '.result.attachments[].env[]'
```

**Output**

Shape from the server's schema; not captured on a live system.

```text
ACCESS KEY            STORE  BUCKET   ACCESS     PREFIX  ENDPOINT
AKIAIOSFODNN7EXAMPLE  media  uploads  readwrite  S3      http://192.0.2.12:9000
```

`STORE` is the store's name, or its id when the store no longer exists.

```json
{
  "result": {
    "attachments": [
      {
        "access_key": "AKIAIOSFODNN7EXAMPLE",
        "store_id": "<store-id>",
        "bucket": "uploads",
        "access": "readwrite",
        "prefix": "S3",
        "vault_item_id": "<vault-item-id>",
        "env": {
          "endpoint": "S3_ENDPOINT",
          "region": "S3_REGION",
          "bucket": "S3_BUCKET",
          "access_key": "S3_ACCESS_KEY_ID",
          "secret_key": "S3_SECRET_ACCESS_KEY"
        },
        "store_name": "media",
        "endpoint": "http://192.0.2.12:9000"
      }
    ],
    "can_change": true
  },
  "schema": "forge.cli/v1"
}
```

| Field | Description |
| --- | --- |
| `access_key` | The access key id issued for this attachment. Pass it to [`forge services storage detach`](#forge-services-storage-detach) |
| `access` | `read` or `readwrite` |
| `prefix` | The prefix of the injected variables |
| `vault_item_id` | The vault item that holds the key's secret |
| `env` | The names of the variables, not their values. The secret is never in the answer |
| `store_name`, `endpoint` | The store's name and S3 endpoint; `null` when the store no longer exists |
| `can_change` | Whether you hold `deploy` on the environment. Attaching a bucket of a store that does not belong to the environment also needs `storage.manage` |

Exit codes: `0` success; `2` an ambiguous name; `3` not signed in; `4` no
`view`; `6` no such service; `7` the service belongs to a Compose
stack.

**See also**: [`forge services storage attach`](#forge-services-storage-attach),
[Attach a bucket to a service](/docs/guides/attach-bucket-to-service/).

### forge services storage attach

Attaches a bucket of a managed store to a service. Forge issues an
access key limited to that bucket and stores its secret in the vault; the
secret is never printed. At its next deploy the service gets five
variables: `<PREFIX>_ENDPOINT`, `<PREFIX>_REGION`, `<PREFIX>_BUCKET`,
`<PREFIX>_ACCESS_KEY_ID` and `<PREFIX>_SECRET_ACCESS_KEY`. The command does
not ask first: it only adds access. Audited as `service.storage.attach`.

Needs `deploy` on the environment. It also needs `storage.manage` on the
store, with an MFA-verified session, unless the store belongs to the
service's environment (see
[`forge storage owner`](/docs/reference/cli/storage/#forge-storage-owner)) and
the bucket is not attached to a service of another environment.

**Synopsis**

```text
forge services storage attach [OPTIONS] --store <STORE> --bucket <BUCKET> <SERVICE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | service id or name | — | yes | The service | `web` |
| `--store <STORE>` | store id, name or slug | — | yes | A managed store. Connected (external) stores issue no keys | `--store media` |
| `--bucket <BUCKET>` | string | — | yes | The bucket to attach: 3 to 63 lower-case letters, digits, dots and hyphens. It must exist unless you pass `--create-bucket` | `--bucket uploads` |
| `--create-bucket` | flag | off | no | Create the bucket when it does not exist yet | `--create-bucket` |
| `--access <ACCESS>` | enum: `read`, `readwrite` | `readwrite` | no | What the service may do in the bucket | `--access read` |
| `--prefix <PREFIX>` | string | `S3` | no | Prefix of the injected variables: an upper-case letter, then upper-case letters, digits and underscores, at most 24 characters, not ending in `_`. Use a second prefix for a second bucket | `--prefix MEDIA` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the service's name up in this environment only | `--environment shop-prod` |

**Examples**

```sh
# Attach a bucket, creating it when it is missing
forge services storage attach web --store media --bucket uploads --create-bucket

# A second bucket, read-only, under its own prefix
forge services storage attach web --store media --bucket reports --access read --prefix REPORTS

# Give the service the variables
forge services deploy web --wait --yes
```

**Output**

The table output prints one line on stderr, then the service's
attachments:

```text
Bucket uploads of media is attached to web (readwrite). Deploy web to give it the S3_* variables.
ACCESS KEY            STORE  BUCKET   ACCESS     PREFIX  ENDPOINT
AKIAIOSFODNN7EXAMPLE  media  uploads  readwrite  S3      http://192.0.2.12:9000
```

With `-o json`, `result` has the shape shown under
[`forge services storage list`](#forge-services-storage-list) and the stderr line is
not printed. Shape from the server's schema; not captured on a live system.

Exit codes: `0` attached; `2` an invalid bucket name or prefix, an `--access`
other than `read` or `readwrite`, or an ambiguous name; `3` not signed in;
`4` no `deploy` on the environment, or no `storage.manage` on the store; `5`
an MFA-verified session is needed; `6` no such service, store or
bucket; `7` the prefix is already used by another attachment, the
service already defines one of the five variables, the bucket is
attached to a service of another environment and you lack `storage.manage`
(`bucket_attached_elsewhere`), the store is a connected store, or the
service belongs to a Compose stack; `9` Forge cannot reach the store.

**See also**: [`forge services storage list`](#forge-services-storage-list),
[`forge storage key-create`](/docs/reference/cli/storage/#forge-storage-key-create),
[Attach a bucket to a service](/docs/guides/attach-bucket-to-service/).

### forge services storage detach

Detaches a bucket from a service. The access key is revoked at once,
so a running instance loses access to the bucket straight away; the
variables disappear with the next deploy. The bucket and its objects stay.
Asks first unless you pass `--yes`. Needs `deploy` on the environment, whoever
owns the store. Audited as `service.storage.detach`.

**Synopsis**

```text
forge services storage detach [OPTIONS] <SERVICE> <ACCESS_KEY>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | service id or name | — | yes | The service | `web` |
| `<access-key>` | string | — | yes | The attachment's access key, from [`forge services storage list`](#forge-services-storage-list) | `AKIAIOSFODNN7EXAMPLE` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the service's name up in this environment only | `--environment shop-prod` |
| `--yes` | flag | off | no | Skip the confirmation prompt. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

:::caution
The key stops working when the command returns, before the next deploy.
Deploy a version of the service that no longer needs the bucket first.
:::

**Examples**

```sh
# Detach a bucket (asks for confirmation)
forge services storage detach web AKIAIOSFODNN7EXAMPLE

# Detach every bucket of a service in a script
forge -o json services storage list web | jq -r '.result.attachments[].access_key' \
  | while read -r key; do forge services storage detach web "$key" --yes; done
```

**Output**

The attachments that remain, in the shape shown under
[`forge services storage list`](#forge-services-storage-list). The table output of an
service with no attachment left prints only the header.

Exit codes: `0` detached; `1` you did not answer `yes`; `2` an ambiguous
name, or no `--yes` where no prompt is possible; `3` not signed in; `4` no
`deploy`; `6` no such service, or no bucket is attached to it with that
key; `7` the service belongs to a Compose stack; `9` Forge cannot reach
the store to revoke the key.

**See also**: [`forge services storage list`](#forge-services-storage-list),
[`forge storage keys`](/docs/reference/cli/storage/#forge-storage-keys).

### forge services logs

Prints the last `--tail` log lines of the service on one node (the
systemd journal for a systemd service, Docker logs for a container). With
`--follow` it keeps printing new lines until you press Ctrl+C: each segment
runs about 50 seconds on the node and the next one continues where it
stopped, so following survives restarts. Needs `view` on the environment.

**Synopsis**

```text
forge services logs [OPTIONS] --node <NODE> <SERVICE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | service id or name | — | yes | The service | `xcweb` |
| `--node <NODE>` | node id, host name or display name | — | yes | The instance's node | `--node lab-b` |
| `--tail <N>` | integer, 0–5000 | `200` | no | Lines to print first | `--tail 50` |
| `--follow` | flag | off | no | Keep printing new lines (Ctrl+C stops). With `-o json`, one segment | `--follow` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab-test` |

**Examples**

```sh
forge services logs xcweb --node lab-b --tail 50
forge services logs xcweb --node lab-b --follow
```

**Output**

The log lines as plain text. With `-o json`, the action result
`{job_id, status, result}`.

Exit codes: `0` success; `3` not signed in; `4` no `view`; `6` no such
service, or it is not placed on the node; `10` the node could not read
the logs.

### forge services remove

Takes the service off one node: stops it there and removes its unit or
container (the folder and data stay). Asks first unless you pass `--yes`.
Needs `deploy` on the environment.

**Synopsis**

```text
forge services remove [OPTIONS] --node <NODE> <SERVICE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | service id or name | — | yes | The service | `xcweb` |
| `--node <NODE>` | node id, host name or display name | — | yes | The node to take it off | `--node lab-c` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab-test` |
| `--yes` | flag | off | no | Skip the confirmation prompt | `--yes` |

**Examples**

```sh
forge services remove xcweb --node lab-c --yes
```

**Output**

The action result as a key/value list (`job_id`, `status`). The server waits
up to 25 seconds; `status: running` means the job continues (follow it with
`forge jobs watch <job_id>`).

Exit codes: `0` success; `1` you did not answer `yes`; `3` not signed in; `4`
no `deploy`; `6` not placed on the node; `10` the removal failed.

### forge services files ls

Lists a folder of the service's app root on every node it is placed on,
or on `--node`. The CLI looks up the app root on each node (no root id
needed) and prints `== <node>` before each node's listing. Needs `file.read`
on the app root.

**Synopsis**

```text
forge services files [--node <NODE>]... <SERVICE> ls [PATH] [-a]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | service id or name | — | yes | The service (before `ls`) | `xcweb` |
| `--node <NODE>` | node id, host name or display name; repeatable | every node it is placed on | no | Only these nodes (before `ls`) | `--node lab-b` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only (before `ls`) | `--environment lab-test` |
| `<path>` | path in the app root | `/` | no | The folder | `/site` |
| `--all`, `-a` | flag | off | no | Include hidden entries | `-a` |

**Examples**

```sh
forge services files xcweb ls /site
```

**Output**

The same table as [`forge nodes files ls`](/docs/reference/cli/nodes-files/#forge-nodes-files-ls), once per node.

Exit codes: `0` every node succeeded; `1` a node failed or has no app root
for the service (the message names each node, code `partial_failure`);
`2` the service is placed on no node; `3` not signed in; `6` no such
service.

### forge services files upload

Uploads one local file to the same path in the app root on every node the
service is placed on (or `--node`), with the checks of
[`forge nodes files upload`](/docs/reference/cli/nodes-files/#forge-nodes-files-upload).
Use it for deploys from CI. Needs `file.write` on the app root, with an
MFA-verified session.

**Synopsis**

```text
forge services files [--node <NODE>]... <SERVICE> upload [--overwrite] <LOCAL> <REMOTE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | service id or name | — | yes | The service (before `upload`) | `xcweb` |
| `--node <NODE>` | node id, host name or display name; repeatable | every node it is placed on | no | Only these nodes | `--node lab-b` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab-test` |
| `<local>` | local path | — | yes | The file to send | `./xcweb.zip` |
| `<remote>` | path in the app root | — | yes | Where it goes | `/xcweb.zip` |
| `--overwrite` | flag | off | no | Replace an existing file | `--overwrite` |

**Examples**

```sh
forge services files xcweb upload ./xcweb.zip /xcweb.zip --overwrite
forge services files xcweb extract /xcweb.zip /site-new
```

**Output**

The upload result of each node, as in `forge nodes files upload`.

Exit codes: as for [`forge services files ls`](#forge-services-files-ls).

### forge services files extract

Unpacks an archive that is already in the app root into a new folder on
every node (or `--node`), as
[`forge nodes files extract`](/docs/reference/cli/nodes-files/#forge-nodes-files-extract)
does. Needs `file.write` on the app root, with an MFA-verified session.

**Synopsis**

```text
forge services files [--node <NODE>]... <SERVICE> extract <ARCHIVE> <DEST>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | service id or name | — | yes | The service (before `extract`) | `xcweb` |
| `--node <NODE>` | node id, host name or display name; repeatable | every node it is placed on | no | Only these nodes | `--node lab-b` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab-test` |
| `<archive>` | path in the app root | — | yes | The `.zip` or `.tar.gz` to unpack | `/xcweb.zip` |
| `<dest>` | path in the app root | — | yes | A new folder (or an existing one with `--replace`) | `/site-new` |
| `--replace` | flag | off | no | Replace an existing folder atomically (needs `file.delete`) | `--replace` |

**Examples**

```sh
forge services files xcweb extract /xcweb.zip /site-new
```

**Output**

Each node's extract result (`entries`, `bytes`).

Exit codes: as for [`forge services files ls`](#forge-services-files-ls).

### forge services files mkdir

Creates a folder in the app root on every node (or `--node`). Needs
`file.write` on the app root, with an MFA-verified session.

**Synopsis**

```text
forge services files [--node <NODE>]... <SERVICE> mkdir [-p] <PATH>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | service id or name | — | yes | The service (before `mkdir`) | `xcweb` |
| `--node <NODE>` | node id, host name or display name; repeatable | every node it is placed on | no | Only these nodes | `--node lab-b` |
| `--environment <ENVIRONMENT>` | environment id or name | — | no | Look the name up in this environment only | `--environment lab-test` |
| `<path>` | path in the app root | — | yes | The folder to create | `/site/assets` |
| `--parents`, `-p` | flag | off | no | Create missing parent folders too | `-p` |

**Examples**

```sh
forge services files xcweb mkdir -p /site/assets
```

**Output**

Each node's result, as in `forge nodes files mkdir`.

Exit codes: as for [`forge services files ls`](#forge-services-files-ls).

## Service definition

`forge services create --file` and `forge services update --file` read the body of
[`POST /services`](/docs/reference/rest-api/#services) as JSON. `--file`
takes a path, `@<path>`, or `@-` to read from stdin. The CLI checks the body
against the server's schema and exits with `2` (`invalid_body`) on a
mismatch.

| Field | Type | Default | Flag | Description |
| --- | --- | --- | --- | --- |
| `environment_id` | environment id | — (required for create) | `--environment` | The environment. Ignored by `update` |
| `name` | string | — (required for create) | `--name` | Lower-case letters, digits and dashes, at most 40. Ignored by `update` |
| `runtime` | `systemd` or `container` | — (required) | `--runtime` | What runs |
| `command` | string | `""` | `--command` | One line. Required for a systemd service; for a container it replaces the image's command |
| `image` | string | `""` | `--image` | Required for a container |
| `ports` | list of strings | `[]` | `--port` | Container: `[IP:]HOST:CONTAINER[/tcp\|udp]` |
| `volumes` | list of strings | `[]` | `--volume` | Container: `HOST_PATH:CONTAINER_PATH[:ro]` (a host folder) or `VOLUME_NAME:CONTAINER_PATH[:ro]` (a named Docker volume, which never needs `stack.privileged`). See [Configuration sections](/docs/guides/services/#configuration-sections) for which host paths need `stack.privileged` |
| `listen_port` | integer, 1 to 65535, or `null` | `null` | `--listen-port` | Native service only: the TCP port it listens on |
| `restart` | bool | `true` | `--restart` | Restart it when it stops |
| `env` | list of `{key, value, secret}` | `[]` | `--env`, `--secret-env`, `--unset-env` | Variables. `secret` defaults to `false`. In an update, a secret without a `value` keeps its stored value |
| `working_dir` | string | `""` | none | Systemd service: the working directory |
| `env_files` | list of package ids | `[]` | none | Installed App Store packages whose `/etc/forge/pkgs/<package>.env` is loaded first |
| `dependencies` | list of `{package, version, params}` | `[]` | none | Runtimes the service needs, at most 10. `forge services deploy` installs the missing ones |
| `cpu_millis` | integer | `0` (no limit) | none | CPU limit in thousandths of a core |
| `memory_mb` | integer | `0` (no limit) | none | Memory limit in MB |
| `entrypoint` | string | `""` | none | Container only |
| `user` | string | `""` | none | Container only: a name, `uid` or `uid:gid` |
| `hostname` | string | `""` | none | Container only: letters, digits and dashes |
| `labels` | list of `[key, value]` pairs | `[]` | none | Container only, at most 50. `forge.*` is reserved |
| `extra_hosts` | list of `name:ipv4` | `[]` | none | Container only: extra `/etc/hosts` entries |
| `healthcheck` | `{command, interval_secs, timeout_secs, retries, start_period_secs}` or `null` | `null` | none | Container only: Docker `HEALTHCHECK`. Defaults: every 30 s, timeout 5 s, 3 retries, start period 0 |
| `stop_timeout_secs` | integer, seconds | `0` (not set) | none | Container only: the stop timeout |
| `read_only` | bool | `false` | none | Container only: read-only root filesystem |
| `log_max_mb` | integer, MB | `0` (not set) | none | Container only: log rotation size |
| `mesh` | bool | `true` | none | Container only: `false` keeps the container off the node's [cluster container network](/docs/guides/cluster-network/#container-network) |
| `on_node_failure` | `move`, `alert`, or absent | absent | none | What the [service reconciler](/docs/guides/services/#keep-services-running) does when the node is offline past `services.node_grace_secs` or drained. Absent: `move` for a container with no volumes, devices or env files, `alert` otherwise. A systemd service is never moved. Set it with `--file` |
| `restart_policy` | `no`, `always`, `unless-stopped`, `on-failure`, or absent | absent | `--restart-policy` | When the service is restarted. When it is absent, `restart` decides: `true` is `unless-stopped`. A native service maps it onto systemd `Restart=` |
| `restart_max_attempts` | integer, 0 to 1000 | `0` (no limit) | `--restart-max-attempts` | `on-failure` only: stop restarting after this many restarts. A clean exit (status 0) is never restarted. A deploy or a manual start begins a new count |
| `cap_add` | list of capability names | `[]` | `--cap-add` | Container only: Linux capabilities to add, such as `NET_ADMIN`. Deploying needs `stack.privileged` |
| `cap_drop` | list of capability names | `[]` | `--cap-drop` | Container only: Linux capabilities to drop; `ALL` drops every one |
| `ulimits` | list of `{name, soft, hard}` | `[]` | none | Container only: resource limits such as `nofile`. `-1` means unlimited, and `soft` may not exceed `hard` |
| `tmpfs` | list of `{target, size_mb}` | `[]` | `--tmpfs` | Container only: tmpfs mounts, at most 20. A `size_mb` of `0` means Docker's default |
| `dns` | list of IP addresses | `[]` | `--dns` | Container only: DNS servers, at most 8 |
| `sysctls` | list of `[key, value]` pairs | `[]` | none | Container only: kernel parameters such as `net.core.somaxconn`. Deploying needs `stack.privileged` |
| `devices` | list of `{host, container, permissions}` | `[]` | none | Container only: host devices under `/dev/`. `permissions` is drawn from `rwm` and defaults to `rwm`. Deploying needs `stack.privileged` |
| `init` | bool | `false` | `--init` | Container only: run Docker's init as PID 1 |
| `networks` | list of Docker network names | `[]` | `--network` | Container only: existing networks on the node to connect to as well, at most 10 |
| `version` | integer | absent | none | Update only: the `version` you read. If the service was saved since then, the update fails with `409` |

Example `app.json` for a container:

```json
{
  "environment_id": "<environment-id>",
  "name": "web",
  "runtime": "container",
  "image": "nginx:1.27",
  "ports": ["8080:80"],
  "env": [{ "key": "MODE", "value": "prod" }],
  "memory_mb": 256,
  "healthcheck": { "command": "curl -fsS http://localhost/ || exit 1" }
}
```

Example `app.json` for a native service with a runtime:

```json
{
  "environment_id": "<environment-id>",
  "name": "api",
  "runtime": "systemd",
  "command": "python3 -m http.server 8080",
  "listen_port": 8080,
  "dependencies": [{ "package": "python", "version": "system" }]
}
```

Health checks beyond the container `healthcheck` are not part of the
definition; an update keeps them. See
[`forge services health`](#forge-services-health).

## Docker Swarm

For a service in a Swarm-mode cluster (see
[Docker Swarm](/docs/guides/docker-swarm/)). Changes need `deploy` on the
service and start a job; add `--wait` to wait for it.

### forge services swarm status

The Swarm service: settings, tasks, HA report

**Synopsis**

```text
forge services swarm status [OPTIONS] <SERVICE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | string | - | yes | Service id or name | - |
| `--environment` | string | - | no | The service's environment (id or name), when the name is ambiguous | - |

**Examples**

```sh
# Replicas and tasks of a Swarm service
forge services swarm status <service> --environment lab
```

### forge services swarm deploy

Apply the current spec to the swarm (skipped when unchanged)

**Synopsis**

```text
forge services swarm deploy [OPTIONS] <SERVICE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | string | - | yes | Service id or name | - |
| `--environment` | string | - | no | The service's environment (id or name), when the name is ambiguous | - |
| `--wait` | flag | - | no | Wait for the job to finish (exit 10 when it fails) | - |
| `--timeout` | integer | `900` | no | Seconds `--wait` waits | - |

**Examples**

```sh
# Deploy the service to the swarm and wait
forge services swarm deploy <service> --environment lab --wait
```

### forge services swarm scale

Set the replica count (0 to 1000)

**Synopsis**

```text
forge services swarm scale [OPTIONS] <SERVICE> <REPLICAS>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | string | - | yes | Service id or name | - |
| `<replicas>` | string | - | yes | Replica count (0 to 1000) | - |
| `--environment` | string | - | no | The service's environment (id or name), when the name is ambiguous | - |
| `--wait` | flag | - | no | Wait for the job to finish (exit 10 when it fails) | - |
| `--timeout` | integer | `900` | no | Seconds `--wait` waits | - |

**Examples**

```sh
# Run five replicas
forge services swarm scale <service> 5 --wait
# Stop every replica without removing the service
forge services swarm scale <service> 0 --wait
```

### forge services swarm restart

Restart every task, one batch at a time

**Synopsis**

```text
forge services swarm restart [OPTIONS] <SERVICE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | string | - | yes | Service id or name | - |
| `--environment` | string | - | no | The service's environment (id or name), when the name is ambiguous | - |
| `--wait` | flag | - | no | Wait for the job to finish (exit 10 when it fails) | - |
| `--timeout` | integer | `900` | no | Seconds `--wait` waits | - |

**Examples**

```sh
# Restart every task, one update at a time
forge services swarm restart <service> --wait
```

### forge services swarm rollback

Go back to the previous release

**Synopsis**

```text
forge services swarm rollback [OPTIONS] <SERVICE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | string | - | yes | Service id or name | - |
| `--environment` | string | - | no | The service's environment (id or name), when the name is ambiguous | - |
| `--yes` | flag | - | no | Skip the confirmation prompt | - |
| `--wait` | flag | - | no | Wait for the job to finish (exit 10 when it fails) | - |
| `--timeout` | integer | `900` | no | Seconds `--wait` waits | - |

**Examples**

```sh
# Go back to the previous spec
forge services swarm rollback <service> --yes --wait
```

### forge services swarm remove

Remove the Swarm service (the Forge service stays)

**Synopsis**

```text
forge services swarm remove [OPTIONS] <SERVICE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | string | - | yes | Service id or name | - |
| `--environment` | string | - | no | The service's environment (id or name), when the name is ambiguous | - |
| `--yes` | flag | - | no | Skip the confirmation prompt | - |
| `--wait` | flag | - | no | Wait for the job to finish (exit 10 when it fails) | - |
| `--timeout` | integer | `900` | no | Seconds `--wait` waits | - |

**Examples**

```sh
# Remove the service from the swarm
forge services swarm remove <service> --environment lab --yes --wait
```

### forge services swarm logs

The last lines of the service's logs

**Synopsis**

```text
forge services swarm logs [OPTIONS] <SERVICE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<service>` | string | - | yes | Service id or name | - |
| `--environment` | string | - | no | The service's environment (id or name), when the name is ambiguous | - |
| `--tail` | integer | `200` | no | Lines from the end | - |

**Examples**

```sh
# The last 200 log lines of every task
forge services swarm logs <service> --tail 200
```

