---
title: Projects, environments and services
description: How Forge groups what you run into projects, their environments and the services inside them, and how that maps to clusters, roles and the API.
sidebar:
  order: 3
---

Forge arranges what you run in three levels: a **project** holds
**environments**, and an environment holds **services**. Knowing the levels
tells you where a service runs, who may change it and how two copies of it
on different nodes become one highly available service.

```diagram
alt: Project, environments and services
tree:
  - label: Project
    note: "a product, e.g. shop"
    tone: accent
    children:
      - label: "Environment: test"
        note: runs in cluster lab
        children:
          - { label: "Service: api", note: systemd on lab-b }
          - { label: "Service: web", note: container on lab-c }
          - { label: "Service: seq", note: container on lab-c }
      - label: "Environment: staging"
        note: runs in cluster lab
        children:
          - "Service: api"
          - "Service: web"
          - "Service: postgres"
```

## Project

A project is a product you host: an online shop, an internal portal, a data
pipeline. It is a name and a description. It owns environments and, through
them, services. A project does not run anywhere itself.

Every environment belongs to exactly one project, and every service belongs
to exactly one environment, so the tree has no loose parts.

## Environment

An environment is one deployment of the project: `test`, `staging`, `uat`,
`production`, or whatever your team calls its stages. The name is free text
that you choose; Forge does not prescribe it. An environment runs in one
[cluster](/docs/concepts/clusters/): its services are placed on that
cluster's nodes, and its internal names end in that cluster's domain
(`<service>.<environment>.<cluster>.forge.internal`). Several environments
of one project may share a cluster, as `test` and `staging` do above, or
each may have its own. Names are unique within a cluster.

Mark an environment **protected** when it is production-like. The console
shows the flag; no guardrail enforces it yet.

## Service

A service is one thing that runs inside an environment: the API, the web UI,
a log collector such as Seq, a database, a worker. Its **runtime** is one of
two kinds:

| Runtime | What runs | On the node |
| --- | --- | --- |
| `systemd` | A command under systemd (`gunicorn`, `dotnet app.dll`, `node server.js`) | Unit `forge-app-<environment>-<service>.service`, as its own system account |
| `container` | A Docker image with ports, volumes and environment variables | Container `forge-app-<environment>-<service>`, visible on its own [container page](/docs/guides/containers/) |

Services deployed from a [compose stack](/docs/concepts/compose-stacks/) are
services too, registered in the stack's environment.

:::note
A service is a long-running workload. Forge has no scheduled or one-shot
service runtime. The durable [jobs](/docs/concepts/jobs/) engine is a
separate thing: it carries out deploys and other operations.
:::

A service is placed on one or more nodes of the environment's cluster. The
first node is active. Each further node is either a **replica**, which also
serves traffic, or a **standby**, which takes over when every replica is
down. An [ingress endpoint](/docs/guides/ingress/) that targets the service
sends traffic to all replicas with the balancing you pick, and to standbys
only as backups. That is how one service becomes highly available: run it
on two nodes in the same environment and put an endpoint in front.

## How the levels map

The same word names each level in the console, the API, the CLI, Cedar and
the audit log:

| Level | API | CLI | Cedar entity | Access scope | Audit actions | Unique within |
| --- | --- | --- | --- | --- | --- | --- |
| Project | `/api/v1/projects` | [`forge projects`](/docs/reference/cli/projects/) | `Project` | `project:<id>` | `project.*` | the workspace |
| Environment | `/api/v1/environments` | [`forge environments`](/docs/reference/cli/tenants-environments/) | `Environment` | `environment:<id>` | `environment.*` | its cluster |
| Service | `/api/v1/services` | [`forge services`](/docs/reference/cli/services/) | `Service` | `service:<id>` | `service.*` | its environment |

## Roles

A role binding's scope can be the whole workspace, a cluster, a project, an
environment or a service. A binding on a project covers all of its
environments and their services; a binding on an environment covers that
environment and its services. A binding on a cluster covers the cluster's
nodes, not the environments placed in it, because the same environment
could move to another cluster. See
[Authorization](/docs/concepts/authorization/) for the model and
[Roles and access](/docs/guides/roles-and-access/) for the tasks.

## Trade-offs

- **An environment belongs to one project.** Sharing one environment
  between two products would blur ownership and roles. Put shared
  infrastructure, such as a database both products use, in its own
  project.
- **Environments pick a cluster, projects do not.** This lets `test` run on
  a small cluster and `production` on a large one under the same project.
- **Two runtimes only.** Keeping services long-running keeps health checks,
  placement and ingress simple. Run periodic work inside a service or with
  your own scheduler.

## Next steps

- [Create and deploy a service](/docs/guides/services/)
- [Clusters](/docs/concepts/clusters/)
- [Ingress](/docs/guides/ingress/) for load balancing across replicas
