---
title: Deployment YAML (forge/v1)
description: Every field of the forge.deploy.yaml Deployment document, with types, defaults, limits and a complete example.
sidebar:
  order: 9
---

This page lists every field of a `forge/v1` `Deployment` document
(`forge.deploy.yaml`). One document describes one release of one service in
one environment. For how Forge rolls it out, see
[Deployments and releases](/docs/concepts/deployments/); for the task, see
[Deploy a service from YAML](/docs/guides/deploy-with-yaml/).

The console calls the deployable unit a **service**; the API and the store
call it an `service`. `metadata.name` is the service's name.

## Document rules

| Rule | Value |
| --- | --- |
| Format | YAML 1.2, one document. `yes`, `no`, `on` and `off` are strings, not booleans |
| Largest document | 256 KiB |
| Parser limits | Depth 32, 20 000 nodes, 200 anchors, 200 aliases |
| Unknown fields | Refused everywhere, except inside `spec.template`, `spec.health.checks` and `spec.overrides` |
| Key style | camelCase. Keys inside `spec.template` and `spec.health.checks` may be camelCase or snake_case; Forge converts them to snake_case |
| Errors | Reported with a dotted path, for example `spec.strategy.steps[1].weight: weights rise from 1 to 100` |

## Durations

Fields of type *duration* take a whole number of seconds or a string with a
unit.

| Written as | Means |
| --- | --- |
| `90` | 90 seconds |
| `"30s"` | 30 seconds |
| `"5m"` | 5 minutes |
| `"2h"` | 2 hours |
| `"1d"` | 1 day |

Fractions and combined units (`1h30m`) are refused.

## Top level

| Field | Type | Default | Required | Description |
| --- | --- | --- | --- | --- |
| `apiVersion` | string | — | yes | Must be `forge/v1` |
| `kind` | string | — | yes | Must be `Deployment` |
| `metadata` | object | — | yes | See [metadata](#metadata) |
| `spec` | object | — | yes | See [spec](#spec) |

## metadata

| Field | Type | Default | Required | Description |
| --- | --- | --- | --- | --- |
| `name` | string | — | yes | The service's name: lowercase letters, digits and dashes, at most 40 characters. A service with this name in the environment is updated; otherwise the first apply creates it |
| `environment` | string | — | yes | The environment's slug (`prod`, `staging`, …), at most 40 characters. `forge deploy apply --environment` and the API's `environment` field replace it |
| `description` | string | `""` | no | Free text shown with the release, at most 500 characters, no NUL or carriage return |

## spec

| Field | Type | Default | Required | Description |
| --- | --- | --- | --- | --- |
| `runtime` | enum: `container`, `systemd` | — | yes | `container` runs an image; `systemd` is a long-running native process under systemd. A release cannot change the runtime of an existing service |
| `artifact` | object | — | yes | What runs. See [artifact](#specartifact) |
| `template` | object | `{}` | no | The service's runtime fields. See [template](#spectemplate) |
| `env` | map string → string | `{}` | no | Plain environment variables. Names: letters, digits and `_`, not starting with a digit |
| `secrets` | list of strings | `[]` | no | Names of secret variables the service already has. See [secrets](#specsecrets) |
| `placement` | object | the service's current nodes | on first apply | Where it runs. See [placement](#specplacement) |
| `strategy` | object | rolling, start-first, batches of 1 | no | How the release replaces the old one. See [strategy](#specstrategy) |
| `health` | object | see below | no | Readiness gate and checks. See [health](#spechealth) |
| `traffic` | object | see below | no | Draining. See [traffic](#spectraffic) |
| `rollback` | object | see below | no | Automatic rollback and release retention. See [rollback](#specrollback) |
| `overrides` | map environment slug → object | `{}` | no | Per-environment changes. See [overrides](#specoverrides) |

## spec.artifact

Exactly one of `image`, `git` and `archive`. An image needs
`runtime: container`; `git` and `archive` need `runtime: systemd`.

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `image` | string | — | Container image: `registry/repo:tag`, `registry/repo@sha256:<64 hex>` or `registry/repo:tag@sha256:<64 hex>` |
| `digest` | string | — | `sha256:` and 64 lower-case hex digits. Pins `image`. When `image` already carries a digest, the two must match |
| `git.repo` | string | — | `https://` clone URL, at most 2048 characters, no whitespace |
| `git.commit` | string | — | The full 40-character commit SHA, lower case. Branch and tag names are refused |
| `git.path` | string | `""` | Folder inside the repository to run from: relative, no `.` or `..` parts, at most 256 characters of letters, digits, `/`, `_`, `-` and `.` |
| `archive.url` | string | — | `https://` URL of a `.tar.gz`, `.tgz`, `.tar` or `.zip` file (the extension is read from the URL's path, before `?` and `#`) |
| `archive.sha256` | string | — | Lower-case hex SHA-256 of the file (64 digits). The node refuses a file that does not match |
| `archive.stripComponents` | integer | `0` | Leading folders to drop when unpacking, 0–8. GitHub source archives need `1`. For a `.zip`, each stripped level must be a single folder |

**Image pinning.** An image with a digest is pulled by digest on every
node. An image with only a tag is resolved to a digest when the first node
pulls it. Forge records that digest on the release, and every other node,
and every later redeploy of the release, pulls the same digest.

**Native artifacts.** The node fetches `git` and `archive` artifacts once
per release into `/srv/forge/apps/<environment>/<service>/releases/<number>`,
as the service's own account, and runs the service from there (plus
`git.path`) unless `template.workingDir` is set. Git artifacts need `git` on
the node; archives need `curl` and `prlimit` (from `util-linux`), plus `tar`
or `unzip`. Archives are limited to 2 GiB. The limit holds while the file
downloads, also when the server does not send its size. The node checks the
SHA-256 before it unpacks anything, and it reads the file in small chunks,
so the agent's memory use does not grow with the archive's size. The node
keeps the five newest release folders.

## spec.template

The service's runtime fields, as on the
[Services API](/docs/reference/rest-api/#services) and in the
[service definition](/docs/reference/cli/services/#service-definition).
Write them in camelCase (`listenPort`, `cpuMillis`, `memoryMb`,
`workingDir`, `stopTimeoutSecs`) or snake_case. Common ones:

| Key | Runtime | Description |
| --- | --- | --- |
| `command` | systemd | Command line, run by `/bin/sh` |
| `workingDir` | systemd | Working directory (default: the release folder for `git`/`archive`, else the app folder) |
| `listenPort` | systemd | TCP port the process listens on. In a slot, the process gets its own port in `$PORT` instead |
| `ports` | container | `[ip:]host:container[/udp]` mappings |
| `volumes` | container | `host:container[:ro]` bind mounts |
| `restart` | systemd, container | Restart when it exits |
| `cpuMillis`, `memoryMb` | all | Limits (`0` = none) |
| `dependencies` | systemd | App Store runtimes to install first |

These keys are refused in `template`, because another field sets them:
`env` (use `spec.env` and `spec.secrets`), `environment` and `name`
(`metadata`), `runtime` (`spec.runtime`), `image` (`spec.artifact`),
`slot` (set by Forge) and `health` (`spec.health.checks`).

:::caution
A container volume outside `/srv/forge/apps/<environment>/<service>/` and
the node's approved file roots needs `stack.privileged` (an administrator
with MFA) on the environment. Without it, `plan` and `apply` answer `403`.
:::

## spec.secrets

Secret values never appear in the document. List the names of secret
variables here; each must already be set as a secret on the service
(console: service page, **Environment** tab). The release stores the
sealed value as it is when the release is created. A name in `secrets` may
not also be in `env`.

A brand-new service has no secrets yet, so a first apply that lists
`secrets` fails. Apply without them, set the secrets, then apply again.

## spec.placement

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `nodes` | list of strings | `[]` | Node ids, host names or display names (case-insensitive). Every name must match a node of the workspace |
| `nodeSelector` | map string → string | `{}` | Tags a node must all have, written `key: value` (matches the node tag `key:value`) |
| `replicas` | integer | all matching nodes | How many of the matching nodes, 1–200. Nodes the service already runs on come first, then online ones, then by name |

Give `nodes`, `nodeSelector` or both. Leave `placement` out to keep the
service's current nodes; a new service needs it. At most 200 nodes per
deployment. Nodes that the placement adds need `server.manage` on each;
nodes it drops need `server.manage` too, and the service is removed from
them after the rollout succeeds.

## spec.strategy

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `type` | enum: `rolling`, `recreate`, `blue-green`, `canary`, `manual` | `rolling` | See [Strategies](/docs/concepts/deployments/#strategies) |
| `replacement` | enum: `start-first`, `stop-first` | `start-first` | Rolling and manual: start the new copy next to the old one (slots), or take the node out of traffic and replace the copy in place |
| `maxSurge` | integer | `1` | Nodes per batch with `start-first` |
| `maxUnavailable` | integer | `0` (treated as `1`) | Nodes per batch with `stop-first` |
| `batchSize` | integer | — | Nodes per batch; overrides both. At least 1 |
| `steps` | list | `[]` | Canary only, 1–20 entries. See [canary steps](#canary-steps) |
| `approval.required` | boolean | `false` | Wait for an approval before traffic first moves to the new release |
| `approval.timeout` | duration | `"1d"` | Fail the deployment when nobody approves in time, 1s–30d. Applies to every approval of the deployment |
| `allowDowntime` | boolean | `false` | Accept that every running copy is down at once (stop-first when the batch covers every node). Without it the plan is refused |

### Canary steps

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `weight` | integer | — | Percent of traffic on the new release, 1–100. Each step's weight must be higher than the one before |
| `pause` | duration | `0` | How long to hold this weight |
| `approval` | boolean | `false` | Hold here until someone approves |
| `analysis.maxErrorRate` | integer, percent | none | Most 5xx answers (and failed connections) of the new release, in percent of its requests |
| `analysis.maxP95Ms` | integer, ms | none | Slowest acceptable 95th percentile of upstream response time |
| `analysis.minRequests` | integer | `20` | Fewer requests than this: the gate does not judge. At the end of the pause the step goes on with a warning |

Analysis reads the edge nodes' traffic log every 15 seconds during the
pause. A failed gate fails the deployment at once. On a service's first
release, steps below 100 are skipped: there is nothing to compare with.

## spec.health

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `startupTimeout` | duration | `"5m"` | Longest wait for a new copy to become ready, 1s–3h |
| `stabilizationPeriod` | duration | `"30s"` | The copy must then stay ready this long before it gets traffic, at most 1h |
| `checks` | object | the service's own policy | `readiness`, `liveness`, `startup` and `defaultChecks`, as in the [health policy](/docs/reference/health-policy/). Leave it out to keep the service's current checks |

The gate uses the default check (unit active or container running, plus a
TCP connect to the listen port) and each required HTTP and TCP readiness
check of the workload. Checks that run code, reach another host or send
vault-backed headers are left to the node's regular health runner.

## spec.traffic

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `drainTimeout` | duration | `"30s"` | How long a copy taken out of traffic keeps running so requests in flight finish, at most 1h |

## spec.rollback

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `automatic` | boolean | `true` | When the rollout fails after new copies took traffic, start a rollback to the release that served before |
| `retainReleases` | integer | `20` | Releases kept, 1–500. Older ones are deleted after a successful rollout; the current and the previous release are always kept |

## spec.overrides

A map from environment slug to a patch. When the document is applied to an
environment that has an entry, Forge merges that entry over `spec` as a
[JSON merge patch (RFC 7396)](https://www.rfc-editor.org/rfc/rfc7396)
before it reads the document:

- objects merge key by key;
- any other value, lists included, replaces the value in `spec`;
- `null` removes the key.

Write keys as in `spec` (camelCase). The `overrides` entries of other
environments are ignored.

## Complete example

```yaml
apiVersion: forge/v1
kind: Deployment
metadata:
  name: web
  environment: staging
  description: Release built by CI
spec:
  runtime: container
  artifact:
    image: ghcr.io/<org>/web:1.4.2
    digest: sha256:<64-hex-digest>
  template:
    ports: ["8080:80"]
    cpuMillis: 500
    memoryMb: 256
  env:
    LOG_LEVEL: info
  secrets:
    - DATABASE_URL
  placement:
    nodeSelector:
      role: web
    replicas: 2
  strategy:
    type: rolling
    replacement: start-first
    maxSurge: 1
  health:
    startupTimeout: 3m
    stabilizationPeriod: 20s
    checks:
      readiness:
        - name: http
          probe: { type: http, port: 80, path: /healthz }
  traffic:
    drainTimeout: 15s
  rollback:
    automatic: true
    retainReleases: 30
  overrides:
    prod:
      placement:
        nodeSelector:
          role: web
        replicas: 4
      strategy:
        type: canary
        steps:
          - weight: 10
            pause: 5m
            analysis: { maxErrorRate: 1, maxP95Ms: 400 }
          - weight: 50
            pause: 10m
            approval: true
          - weight: 100
        approval:
          required: true
          timeout: 4h
```

A native service from a pinned Git commit:

```yaml
apiVersion: forge/v1
kind: Deployment
metadata:
  name: api
  environment: prod
spec:
  runtime: systemd
  artifact:
    git:
      repo: https://github.com/<org>/<repo>.git
      commit: <40-character-commit-sha>
      path: server
  template:
    command: ./run.sh
    listenPort: 8080
    restart: true
  strategy:
    type: blue-green
```

`run.sh` must listen on `$PORT` when the service runs in slots; see
[Slots](/docs/concepts/deployments/#slots).

## Related

- [Deployments and releases](/docs/concepts/deployments/)
- [Deploy a service from YAML](/docs/guides/deploy-with-yaml/)
- [`forge deploy`](/docs/reference/cli/deploy/)
- [REST API: Releases and deployments](/docs/reference/rest-api/#releases-and-deployments)
