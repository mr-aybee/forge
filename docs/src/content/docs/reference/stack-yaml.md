---
title: Stack YAML (forge/v1)
description: Every field of the forge.stack.yaml Stack document, with types, defaults, rules and a complete example.
sidebar:
  order: 10
---

This page lists every field of a `forge/v1` `Stack` document
(`forge.stack.yaml`). One document describes one compose stack in one
environment: where its compose file is, which nodes run it and how a new
revision rolls out. For the task, see
[Deploy a stack from YAML](/docs/guides/deploy-stack-with-yaml/); for the
commands, see [`forge stacks validate / plan / apply`](/docs/reference/cli/stacks/#stack-documents).

## Document rules

| Rule | Value |
| --- | --- |
| Format | YAML 1.2, one document. `yes`, `no`, `on` and `off` are strings, not booleans |
| Largest document | 256 KiB |
| Parser limits | Depth 32, 20 000 nodes, 200 anchors, 200 aliases |
| Unknown fields | Refused everywhere |
| Key style | camelCase |
| Errors | Reported with a dotted path, for example `spec.rollout.healthTimeout: between 10s and 1h` |
| Files | `spec.compose.file` and `spec.compose.envFile` are read by the CLI, relative to the document's folder, and sent as text. The server never reads files |

Durations take a whole number of seconds or a string with a unit: `90`,
`"30s"`, `"5m"`, `"1h"`.

## Fields

| Field | Type | Required | Default | Rule | Example |
| --- | --- | --- | --- | --- | --- |
| `apiVersion` | string | yes | — | `forge/v1` | `forge/v1` |
| `kind` | string | yes | — | `Stack` | `Stack` |
| `metadata.name` | string | yes | — | Stack name: `a-z`, `0-9`, `-`, `_`, up to 63 characters, starting with a letter or digit. Created on the first apply | `orders` |
| `metadata.environment` | string | yes | — | The environment's slug (its name in lower case, other characters as `-`). `--environment` replaces it | `prod` |
| `spec.compose.file` | string | one of `file` and `inline` | — | Path of the compose file, relative to the document, without `.` or `..` parts | `compose.yaml` |
| `spec.compose.inline` | string | one of `file` and `inline` | — | The compose document itself | see below |
| `spec.compose.envFile` | string | no | none | Path of a `.env` file, same rules as `file`. Sent as text and sealed by the server; values are never returned. Omitted on an update keeps the stored variables | `prod.env` |
| `spec.placement.nodes` | list of strings | no | none | Node hostnames, console names or ids. The first is the stack's default node: services without their own placement run there. With more than one node, at least one service must name its node with `x-forge.node` (or `x-forge.cluster`, or a `node.hostname` constraint); otherwise the document is refused: `placement.nodes lists several nodes; name the node per service with x-forge.node or use placement.cluster`. Exclusive with `cluster` | `[lab-b, lab-c]` |
| `spec.placement.cluster` | string | no | none | Cluster name. Every service without its own placement runs on each node of the cluster, as if it had `x-forge.cluster`. The default node is the cluster's first node by hostname. Exclusive with `nodes` | `c1` |
| `spec.network.mesh` | boolean | no | `true` | `false` keeps every service off the [cluster network](/docs/reference/compose-extensions/#cluster-network) (top-level `x-forge.mesh: false`) | `false` |
| `spec.rollout.order` | `node-by-node` or `all-at-once` | no | `node-by-node` | `node-by-node` deploys one node at a time and stops at the first failed node; `all-at-once` deploys every node together | `all-at-once` |
| `spec.rollout.healthTimeout` | duration | no | `5m` | 10 s to 1 h. How long the rollout waits for a node, plus a 20-minute allowance for pulls | `"2m"` |
| `spec.overrides.<env>` | mapping | no | none | Merged over `spec` when the document is applied to environment `<env>`. Only `placement`, `network`, `rollout` and `compose.envFile` may appear | see below |

Without `spec.placement`, the first apply is refused
(`spec.placement: name the nodes or the cluster the stack runs on`); later
applies keep the stack's default node.

## Overrides

`spec.overrides.<env>` is deep-merged over `spec` (a JSON merge patch): keys
it sets replace the base value, keys it leaves out stay, and `null` removes a
key. To move from `nodes` to `cluster` in one environment, clear `nodes`:

```yaml
spec:
  placement:
    nodes: [lab-a]
  overrides:
    prod:
      placement:
        nodes: null
        cluster: c1
```

## How the document reaches the compose file

`placement.cluster` and `network.mesh: false` are added to the compose text
as a top-level `x-forge` block (`cluster`, `mesh`) before the revision is
saved, so the saved revision deploys the same way from the console. When the
compose file already has a top-level `x-forge`, its `cluster` and `mesh` must
match the document, or the document is refused at that field.

## Example

```yaml
# forge.stack.yaml
apiVersion: forge/v1
kind: Stack
metadata:
  name: orders
  environment: staging
spec:
  compose:
    file: compose.yaml
    envFile: staging.env
  placement:
    nodes: [lab-b]
  rollout:
    order: node-by-node
    healthTimeout: 2m
  overrides:
    prod:
      placement:
        nodes: null
        cluster: c1
      rollout:
        healthTimeout: 5m
      compose:
        envFile: prod.env
```

An inline compose file:

```yaml
spec:
  compose:
    inline: |
      services:
        web:
          image: nginx:1.27
          ports: ["8080:80"]
```
