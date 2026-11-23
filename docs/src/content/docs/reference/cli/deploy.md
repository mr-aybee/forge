---
title: forge deploy
description: Validate, plan and apply forge.deploy.yaml documents, follow deployments, and redeploy, roll back, pause, abort or approve them.
sidebar:
  order: 10
---

`forge deploy` creates releases of a service from a `forge.deploy.yaml`
document and controls their rollouts. For how rollouts work, see
[Deployments and releases](/docs/concepts/deployments/); for the document, see
[Deployment YAML](/docs/reference/deployment-yaml/); for the tasks, see
[Deploy a service from YAML](/docs/guides/deploy-with-yaml/) and
[Deploy from CI](/docs/guides/deploy-from-ci/). Every command takes the
[global flags](/docs/reference/cli/#global-flags).

Who may run what: `validate` needs only a sign-in. `plan`, `apply`,
`redeploy`, `rollback`, `pause`, `resume` and `abort` need `deploy` on the
service (on the environment for a service the document creates), plus
`server.manage` on each node the placement adds or removes and
`stack.privileged` (MFA) for container volumes outside the app folder.
`approve` and `reject` need `deploy.approve` and never work on your own
deployment. `status`, `history`, `releases`, `release` and `slots` need
`view` on the service.

The sample output below is illustrative: node names, ids and digests are
placeholders.

## Commands

| Command | What it does |
| --- | --- |
| [`forge deploy validate`](#forge-deploy-validate) | Checks a document (schema, artifact, policy) without changing anything |
| [`forge deploy plan`](#forge-deploy-plan) | Shows the release, nodes and steps a document would roll out, without starting |
| [`forge deploy apply`](#forge-deploy-apply) | Creates a release from the document and rolls it out |
| [`forge deploy status`](#forge-deploy-status) | One deployment: state, steps and events |
| [`forge deploy history`](#forge-deploy-history) | A service's deployments, newest first |
| [`forge deploy releases`](#forge-deploy-releases) | A service's releases, newest first |
| [`forge deploy release`](#forge-deploy-release) | One release, with the document it was made from |
| [`forge deploy slots`](#forge-deploy-slots) | The copies of a service on each node, their slot, release and weight |
| [`forge deploy redeploy`](#forge-deploy-redeploy) | Rolls out an earlier release again |
| [`forge deploy rollback`](#forge-deploy-rollback) | Goes back to the release before the current one |
| [`forge deploy pause`](#forge-deploy-pause) | Holds a running deployment before its next step |
| [`forge deploy resume`](#forge-deploy-resume) | Continues a paused deployment |
| [`forge deploy abort`](#forge-deploy-abort) | Stops a deployment |
| [`forge deploy approve`](#forge-deploy-approve) | Lets a deployment that waits for an approval go on |
| [`forge deploy reject`](#forge-deploy-reject) | Stops a deployment that waits for an approval |

Exit codes follow the [common exit codes](/docs/reference/cli/#exit-codes):

| Code | Meaning |
| --- | --- |
| `0` | Success. With `--wait` or `--follow`: the release is live |
| `2` | Invalid flags, an unreadable file, an invalid document or a plan Forge refuses (`400`/`422`) |
| `3` | Not signed in |
| `4` | The action is denied, or you tried to approve your own deployment |
| `5` | An MFA-verified session is needed (privileged volumes) |
| `6` | No such service, environment, release, node or deployment |
| `7` | Conflict: the service already has a deployment running, or the deployment is not in a state that accepts the request |
| `8` | `--wait` timed out; the deployment goes on |
| `10` | With `--wait` or `--follow`: the deployment ended `failed`, `aborted` or `rolled_back` |

### Deployment table

`apply`, `status`, `history`, the control commands and the end of
`--wait` print deployments with these columns. `-o json` prints every field
of the [deployment object](/docs/reference/rest-api/#releases-and-deployments).

| Column | Field | Meaning |
| --- | --- | --- |
| `ID` | `id` | Deployment id |
| `RELEASE` | `release_number` | The release it rolls out |
| `FROM` | `from_release` | The release that served when it started (`0`: none) |
| `KIND` | `kind` | `apply`, `rollback` or `auto_rollback` |
| `STRATEGY` | `strategy` | `rolling`, `recreate`, `blue-green`, `canary` or `manual` |
| `STATE` | `state` | `pending`, `running`, `paused`, `awaiting_approval`, `succeeded`, `failed`, `aborted` or `rolled_back` |
| `STEP` | `step` | Steps of the plan done |
| `ERROR` | `error` | Why it failed |

### forge deploy validate

Checks a document without contacting nodes or changing anything: the
schema, the artifact and the rollout policy, with the `overrides` entry of
the target environment merged in. It does not check that the environment,
nodes or secrets exist; `plan` does.

```sh
forge deploy validate [-f <file>] [--environment <slug>]
```

| Flag | Type | Default | Meaning | Example |
| --- | --- | --- | --- | --- |
| `-f`, `--file <FILE>` | path | `forge.deploy.yaml` | The document. `-` reads standard input | `-f deploy/web.yaml` |
| `--environment <SLUG>` | environment slug | `metadata.environment` | Validate for this environment; its `overrides` entry is merged in | `--environment prod` |

```
$ forge -o yaml deploy validate -f forge.deploy.yaml --environment prod
valid: true
service: web
environment: prod
runtime: container
artifact: ghcr.io/<org>/web:1.4.2@sha256:0123456789ab
policy:
  strategy: canary
  replacement: start-first
  batch_size: 1
  ...
```

### forge deploy plan

Shows what `apply` would do, without starting: the release number, the
artifact, the nodes (added and removed), whether it causes downtime and
each step. Needs the same permissions as `apply`.

```sh
forge deploy plan [-f <file>] [--environment <slug>]
```

| Flag | Type | Default | Meaning | Example |
| --- | --- | --- | --- | --- |
| `-f`, `--file <FILE>` | path | `forge.deploy.yaml` | The document. `-` reads standard input | `-f -` |
| `--environment <SLUG>` | environment slug | `metadata.environment` | Plan for this environment; its `overrides` entry is merged in | `--environment staging` |

```
$ forge deploy plan
Service      web in staging
Release      8  ghcr.io/<org>/web:1.4.2@sha256:0123456789ab
Strategy     rolling, side by side
Nodes        <node-1>, <node-2>
Downtime     no

Steps
   1. start the release on <node-1> (slot b)
   2. wait until <node-1> (slot b) is ready and stable
   3. route traffic: <node-1> slot b 100%, <node-1> slot a 0%
   4. wait 30s (the old copies finish their requests)
   5. stop <node-1> (slot a)
   6. start the release on <node-2> (slot b)
   ...
```

A refused plan exits `2` with the reason, for example
`<node-2> offline; bring it back or change the placement`.

### forge deploy apply

Creates a release from the document and starts its rollout. Without
`--wait` it prints the deployment and returns. With `--wait` it prints the
plan, follows the events on standard error and exits `0` when the release
is live, `10` when the rollout failed, was aborted or was rolled back, and
`8` when `--timeout` passes first. Audited as `deployment.apply`.

```sh
forge deploy apply [-f <file>] [--environment <slug>] [--wait] [--timeout <minutes>]
```

| Flag | Type | Default | Meaning | Example |
| --- | --- | --- | --- | --- |
| `-f`, `--file <FILE>` | path | `forge.deploy.yaml` | The document. `-` reads standard input | `-f deploy/web.yaml` |
| `--environment <SLUG>` | environment slug | `metadata.environment` | Apply to this environment; its `overrides` entry is merged in | `--environment prod` |
| `--wait` | flag | off | Follow the rollout until it ends | `--wait` |
| `--timeout <MINUTES>` | integer, minutes | `60` | Longest wait with `--wait` (at least 1) | `--timeout 30` |

```
$ forge deploy apply --environment prod --wait
Service      web in prod
Release      8  ghcr.io/<org>/web:1.4.2@sha256:0123456789ab
...
Deployment <deployment-id>
· rolling out release 8 (canary, 9 steps)
· starting release 8 on <node-1> (slot b)
· <node-1> (slot b) is ready
! waiting for approval: forge deploy approve <deployment-id>
· route traffic: <node-1> slot b 10%, <node-1> slot a 90%
· canary at 10%: 412 requests, 0 errors, p95 87 ms
· release 8 is live
ID               RELEASE  STATE      ERROR
<deployment-id>  8        succeeded  -
```

Exit `7` means the service already has a deployment running.

### forge deploy status

Shows one deployment: the table row, each step (done steps marked `✓`)
and its events. With `--follow` it prints events as they happen and exits
like `apply --wait` (no time limit).

```sh
forge deploy status <deployment> [--follow]
```

| Argument or flag | Type | Default | Meaning | Example |
| --- | --- | --- | --- | --- |
| `<deployment>` | id, required | — | Deployment id | `<deployment-id>` |
| `--follow` | flag | off | Follow it until it ends | `--follow` |

```
$ forge deploy status <deployment-id>
ID               RELEASE  FROM  KIND   STRATEGY  STATE    STEP  ERROR
<deployment-id>  8        7     apply  rolling   running  3     -

Steps
  ✓  1. start the release on <node-1> (slot b)
  ✓  2. wait until <node-1> (slot b) is ready and stable
  ✓  3. route traffic: <node-1> slot b 100%, <node-1> slot a 0%
     4. wait 30s (the old copies finish their requests)
     ...

Events
  info rolling out release 8 (rolling, 10 steps)
  ...
```

### forge deploy history

Lists a service's deployments, newest first.

```sh
forge deploy history <service> [-n <environment>] [--limit <n>]
```

| Argument or flag | Type | Default | Meaning | Example |
| --- | --- | --- | --- | --- |
| `<service>` | name or id, required | — | The service | `web` |
| `-n`, `--environment <ENVIRONMENT>` | environment name or id | — | The environment, when the name is not unique | `-n prod` |
| `--limit <LIMIT>` | integer | `20` | Most entries (the server caps it at 200) | `--limit 5` |

```
$ forge deploy history web -n prod --limit 3
ID      RELEASE  FROM  KIND           STRATEGY  STATE        STEP  ERROR
<id-3>  7        8     auto_rollback  rolling   succeeded    10    -
<id-2>  8        7     apply          canary    rolled_back  4     canary at 10%: 6% of 210 requests failed (at most 1%)
<id-1>  7        6     apply          rolling   succeeded    10    -
```

### forge deploy releases

Lists a service's releases, newest first. `ARTIFACT` is the image with a
short digest, the Git repository with a short commit, or the archive URL.

```sh
forge deploy releases <service> [-n <environment>] [--limit <n>]
```

| Argument or flag | Type | Default | Meaning | Example |
| --- | --- | --- | --- | --- |
| `<service>` | name or id, required | — | The service | `web` |
| `-n`, `--environment <ENVIRONMENT>` | environment name or id | — | The environment, when the name is not unique | `-n prod` |
| `--limit <LIMIT>` | integer | `20` | Most entries (the server caps it at 200) | `--limit 50` |

```
$ forge deploy releases web -n prod
NUMBER  ARTIFACT                                       CREATED        DESCRIPTION
8       ghcr.io/<org>/web:1.4.2@sha256:0123456789ab    1763460000000  Release built by CI
7       ghcr.io/<org>/web:1.4.1@sha256:fedcba987654    1763370000000  -
```

### forge deploy release

Shows one release. In the table format it prints the artifact and the YAML
it was made from; `-o json` prints the whole release.

```sh
forge deploy release <service> <number> [-n <environment>]
```

| Argument or flag | Type | Default | Meaning | Example |
| --- | --- | --- | --- | --- |
| `<service>` | name or id, required | — | The service | `web` |
| `<number>` | integer, required | — | Release number | `7` |
| `-n`, `--environment <ENVIRONMENT>` | environment name or id | — | The environment, when the name is not unique | `-n prod` |

```
$ forge deploy release web 7 -n prod
Release 7 — ghcr.io/<org>/web:1.4.1@sha256:fedcba987654

apiVersion: forge/v1
kind: Deployment
...
```

### forge deploy slots

Shows the current release and each copy of the service: node, slot (empty
for the unslotted instance), release, traffic weight, state (`starting`,
`started`, `ready` or `failed`) and its ports (service port → copy port).

```sh
forge deploy slots <service> [-n <environment>]
```

| Argument or flag | Type | Default | Meaning | Example |
| --- | --- | --- | --- | --- |
| `<service>` | name or id, required | — | The service | `web` |
| `-n`, `--environment <ENVIRONMENT>` | environment name or id | — | The environment, when the name is not unique | `-n prod` |

```
$ forge deploy slots web -n prod
Current release 8
NODE       SLOT  RELEASE  WEIGHT  STATE  PORTS
<node-id>  a     7        0       ready  {"8080":20004}
<node-id>  b     8        100     ready  {"8080":20005}
```

### forge deploy redeploy

Rolls out an existing release again, with that release's own rollout
policy, on the service's current nodes. Audited as `deployment.redeploy`.

```sh
forge deploy redeploy <service> <number> [-n <environment>] [--wait] [--timeout <minutes>]
```

| Argument or flag | Type | Default | Meaning | Example |
| --- | --- | --- | --- | --- |
| `<service>` | name or id, required | — | The service | `web` |
| `<number>` | integer, required | — | Release number | `6` |
| `-n`, `--environment <ENVIRONMENT>` | environment name or id | — | The environment, when the name is not unique | `-n prod` |
| `--wait` | flag | off | Follow the rollout until it ends | `--wait` |
| `--timeout <MINUTES>` | integer, minutes | `60` | Longest wait with `--wait` | `--timeout 20` |

```sh
forge deploy redeploy web 6 -n prod --wait
```

### forge deploy rollback

Rolls out the release that served before the current one (the newest
succeeded deployment of another release), or the release given with
`--to`. It is a deployment of kind `rollback` with that release's own
policy. It does not undo data or schema changes; see
[Failure and automatic rollback](/docs/concepts/deployments/#failure-and-automatic-rollback).
Audited as `deployment.rollback`.

```sh
forge deploy rollback <service> [-n <environment>] [--to <number>] [--wait] [--timeout <minutes>]
```

| Argument or flag | Type | Default | Meaning | Example |
| --- | --- | --- | --- | --- |
| `<service>` | name or id, required | — | The service | `web` |
| `-n`, `--environment <ENVIRONMENT>` | environment name or id | — | The environment, when the name is not unique | `-n prod` |
| `--to <TO>` | integer | the previous release | Release number to go back to; not the current one | `--to 5` |
| `--wait` | flag | off | Follow the rollout until it ends | `--wait` |
| `--timeout <MINUTES>` | integer, minutes | `60` | Longest wait with `--wait` | `--timeout 15` |

```sh
forge deploy rollback web -n prod --wait
```

Exit `2` when there is no earlier release that ran, or `--to` names the
current one.

### forge deploy pause

Asks a running deployment to hold before its next step. A pause during a
wait takes effect after the wait. Audited as `deployment.request`.

```sh
forge deploy pause <deployment>
```

| Argument | Type | Meaning | Example |
| --- | --- | --- | --- |
| `<deployment>` | id, required | Deployment id | `<deployment-id>` |

### forge deploy resume

Continues a paused deployment. Audited as `deployment.request`.

```sh
forge deploy resume <deployment>
```

| Argument | Type | Meaning | Example |
| --- | --- | --- | --- |
| `<deployment>` | id, required | Deployment id | `<deployment-id>` |

### forge deploy abort

Stops a deployment, at once even during a wait. New copies that never took
traffic are removed. An aborted deployment is not rolled back
automatically. Audited as `deployment.request`.

```sh
forge deploy abort <deployment>
```

| Argument | Type | Meaning | Example |
| --- | --- | --- | --- |
| `<deployment>` | id, required | Deployment id | `<deployment-id>` |

### forge deploy approve

Lets a deployment in `awaiting_approval` go on. Needs `deploy.approve` on
the service (`platform_admin`, `workspace_admin`, `operator`); refused
(`4`) for the person who started the deployment. Exit `7` when it is not
waiting for an approval. Audited as `deployment.request`.

```sh
forge deploy approve <deployment>
```

| Argument | Type | Meaning | Example |
| --- | --- | --- | --- |
| `<deployment>` | id, required | Deployment id | `<deployment-id>` |

### forge deploy reject

Stops a deployment in `awaiting_approval`; it ends `aborted`. Same
permission and rules as `approve`. Audited as `deployment.request`.

```sh
forge deploy reject <deployment>
```

| Argument | Type | Meaning | Example |
| --- | --- | --- | --- |
| `<deployment>` | id, required | Deployment id | `<deployment-id>` |
