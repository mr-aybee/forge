---
title: Clusters and nodes
description: How the deployment, clusters, nodes and environments relate.
sidebar:
  order: 2
---

This page explains how clusters, nodes and environments fit together, and how
permissions follow that structure.

```diagram
alt: Deployment, tenants, clusters, nodes and environments
tree:
  - label: Orbyn Forge deployment
    note: one URL = one workspace
    tone: accent
    children:
      - Tenants
      - label: Clusters
        children:
          - { label: Nodes, note: Linux / Windows machines running the agent }
          - { label: "Environments", note: "dev, staging, prod… ── services" }
```

- **The deployment is the workspace.** Each Forge installation, reached at its
  own URL, is one workspace. The console does not show workspaces; everything
  you see belongs to it.
- **Clusters** group nodes by region, purpose or customer (`edge-eu`,
  `windows-workers`). Services and sites run on a cluster.
- **Nodes** are machines with the Forge agent. A node belongs to at most one
  cluster and optionally one environment.
- **Environments** describe what you run. Each environment lives in exactly one
  cluster. Names are unique within a cluster, so `dev` in `edge-eu` and `dev`
  in `windows-workers` are two different environments. Moving an environment to
  another cluster takes its services with it.

Environments created before environments belonged to clusters can be
*unassigned*. They keep working and show as **Unassigned** until someone moves
them into a cluster. New environments always need a cluster.

## Permissions follow the structure

Roles can be granted on everything (the workspace), or on a single tenant,
cluster or environment.

- A **node** in cluster *edge-eu* and environment *prod* inherits from both, so
  either grant gives an operator access to it:
  - operator on cluster *edge-eu*
  - operator on environment *prod*
- An **environment** and its services inherit only from the workspace. A
  role on cluster *edge-eu* does not cover the environments placed in it.

A cluster grant is about the machines; an environment grant is about the
workload. Someone who operates the nodes of a shared cluster does not
automatically get to deploy into every team's environment on it. See
[Authorization](/docs/concepts/authorization/) for the full model.

Moving a node between clusters requires management rights on the destination
cluster. Moving an environment requires `workspace.manage` on the environment and
on the target cluster.

## API

| Method | Path | Permission |
| --- | --- | --- |
| GET / POST | `/clusters` | `view` / `workspace.manage` |
| GET / PUT / DELETE | `/clusters/{id}` | `view` / `workspace.manage` on the cluster |
| GET / POST | `/environments` | `view` (filtered) / `workspace.manage` on the cluster |
| PUT | `/environments/{id}/cluster` | `workspace.manage` on the environment and the target cluster |
| PATCH | `/servers/{id}` `{cluster_id, environment_id}` | `server.manage` (+ `server.enroll` on the target cluster) |

Deleting a cluster unassigns its nodes; nothing is uninstalled. The
[REST API reference](/docs/reference/rest-api/#registry) has the request and
response fields.
