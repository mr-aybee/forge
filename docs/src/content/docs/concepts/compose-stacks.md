---
title: Compose stacks
description: How Forge runs Docker Compose files on nodes, splits them per node, checks them against a security policy, binds ports and registers services.
sidebar:
  order: 11
---

A stack is a Docker Compose file that Forge runs on one or more Linux nodes.
This page explains how a stack is placed, checked, run and exposed. To use
stacks, see [Deploy a compose stack](/docs/guides/compose-stacks/) and
[Create a container](/docs/guides/create-container/). For every `x-forge` key and
policy rule, see [Compose extensions and policy](/docs/reference/compose-extensions/).

## The model

A stack lives in an environment and has a name, a compose document, an optional
`.env` file and a folder on its nodes. **Create container** makes a stack
too: a one-service compose file that Forge generates from the form, so both
paths share the same checks, jobs, revisions and registration.

- **Revisions.** Every saved change to the compose document is a new
  revision with its SHA-256, author and time. You can compare revisions and
  deploy an older one to roll back.
- **`.env`.** Variables are sealed with the vault envelope and never returned
  by the API; only their names are shown. On the node the values are in
  `.env` and, because Forge interpolates before it checks, in
  `compose.yaml`. Both files are `0600` and owned by `root`. Anyone who can
  run `docker inspect` on the node can also read a container's environment.
- **Folder.** By default `/opt/forge/stacks/<environment>/<stack>/` on each
  node, or a folder inside an [approved file root](/docs/guides/approve-file-roots/).

## Placement and split

Each service runs on:

- the node named in `x-forge.node`, or in a Swarm-style constraint
  `deploy.placement.constraints: ["node.hostname == lab-c"]`;
- every node of the cluster named in `x-forge.cluster`;
- otherwise, the node the stack was created from.

Forge splits the document: each node gets a compose file with only its
services, plus the networks, volumes, configs and secrets those services use.
A service cannot depend on a service on another node; Forge refuses that
split. The preview groups services by node.

```diagram
alt: How a Compose stack is deployed
flow:
  direction: column
  steps:
    - { label: "`compose.yaml`", edge: "" }
    - { label: parse, note: "YAML 1.2, merge keys, size and alias limits", edge: "" }
    - { label: "interpolate with the stack's `.env`", edge: "" }
    - { label: place services on nodes, edge: "" }
    - { label: split per node, edge: "" }
    - { label: check each part against the policy, note: server, edge: "" }
    - { label: signed command, edge: "" }
    - { label: node helper checks again, edge: "" }
    - { label: "`docker compose up`", tone: accent }
```

## Running Compose

Forge drives Docker's own `docker compose` plugin; it does not translate
Compose into its own format. The node's root helper runs a fixed command:

```
docker compose -p forge-<environment>-<stack> --project-directory <folder> \
  -f <folder>/compose.yaml up --detach --no-build --remove-orphans --wait ...
```

The helper clears the environment before it runs Compose, and writes a
compose file that is already interpolated and checked, with `$` escaped.
What Compose runs is exactly what was checked. `.env` keys that change how
Compose itself runs (`COMPOSE_*`, `DOCKER_*`) are refused.

After the check, the helper gives every service the node's log listener,
as for a single container: `FORGE_LOG_URL=http://forge-node:5341` in its
`environment` and `forge-node:host-gateway` in its `extra_hosts`, unless
the service sets them itself. The exceptions by `network_mode` are listed
in [Send and search service logs](/docs/guides/application-logs/#2-give-the-key-and-the-url-to-the-service).

Deploys are `stack.action` jobs, one per node: plan → send → wait for the
result → record the state and register the services. Every step is
re-authorized. Cancelling a deploy in **Jobs** stops the running
`docker compose` command on the node; containers it already started keep
running until you deploy again or take the stack down.

## The security policy

A compose file can take over a node. Forge checks every part twice: on the
server before it saves or deploys, and in the node helper before it runs
anything.

On the node, the helper walks every bind source and every file Compose reads
from `/` without following links. It refuses a path that passes through a
link (root-owned system links such as `/var/run` are fine), creates missing
bind folders itself (root, `0755`), and checks the resolved path against the
policy again. Docker resolves a bind source again each time a container
starts, so a link planted later could still redirect it. For that reason a
bind source may never lie inside another read-write bind of the stack, where
a container could plant the link.

| Result | Examples | Who can deploy |
| --- | --- | --- |
| Allowed | images, named volumes, binds inside the stack folder or an approved file root, ports | Anyone with `stack.manage` |
| Needs `stack.privileged` | `privileged`, host network, `pid`/`ipc`/`uts`/`userns: host`, `cap_add`, `devices`, `sysctls`, `security_opt` that turns off seccomp or AppArmor, the Docker socket, binds outside approved roots or on system paths | Administrators, with MFA |
| Never allowed | `build:`, `extends`, `include`, `provider`, unknown service keys, paths with `..` or `~`, `env_file` outside the stack folder, binds or files inside another read-write bind, a read-write bind of the stack folder | Nobody |

:::caution
`stack.privileged` lets a stack control the node, including Docker itself.
Forge flags privileged stacks in the preview and records every exception in
the audit log.
:::

:::note
The link check cannot see changes made after it runs. A process outside the
stack that can write in an approved file root, such as another stack or a
user of the file manager, could still swap a folder for a link before a
container restarts. Approve file roots only for folders that people and
stacks you trust can write.
:::

## Ports are internal by default

A published port binds to the node's address on the
[cluster network](/docs/guides/cluster-network/) when the node's mesh is up,
otherwise to `127.0.0.1`. It never binds to all addresses unless the service
lists the port in `x-forge.public`. Other nodes in the cluster and the
cluster's Nginx edges can reach internal ports; the internet cannot.

## Two legs: stack networks and the cluster network

On a node whose [container network](/docs/guides/cluster-network/#container-network)
is up, each container has two legs:

- **Stack networks.** The networks the compose file defines, or Compose's
  `default`. Services of the same stack on the same node talk here by
  service name, as in plain Compose.
- **The cluster network** (`forge-<interface>`, key `forge-mesh`). An address
  from the node's instance block. Every peer in the WireGuard mesh routes that
  block to the node, and Docker's routed gateway mode forwards it without
  address translation. A container on another node reaches this container at
  this address, and sees the caller's real container address.

```diagram
alt: A container on lab-b reaching a container on lab-c over the cluster network
flow:
  direction: column
  steps:
    - { label: "container on lab-b", note: "instance block of lab-b", edge: "" }
    - { label: "WireGuard mesh", note: "routes each node's block", edge: "" }
    - { label: "container on lab-c", note: "instance block of lab-c", tone: accent }
```

Forge adds the second leg on the node, after the policy check, so the file
that was checked is the file that runs. A service with `network_mode`, or
with `x-forge.mesh: false` on itself or on the stack, keeps one leg.

Why a routed Docker bridge rather than WireGuard per container: the instance
block is already routed by every peer, so nothing in the mesh changes when a
container starts or stops, and Docker assigns the addresses. The trade-off is
the Docker 28 minimum, which introduced routed gateway mode. Nodes with an
older Docker, an older agent, Windows nodes and Swarm clusters keep one leg;
their services are reachable from other nodes only through published ports.

There is no firewall between environments on the cluster network: any
container in the cluster can reach any other container's address. Names for
these addresses are not published yet; use the address the stack status
reports (`mesh_ip`).

## Services and exposure

After a deploy, each service on each node is registered as a service of the
environment (runtime `compose-service`) and shown with its state and health
from `docker compose ps`. These services are managed through their stack;
the services API refuses to change them directly.

**Expose via Nginx** creates a [site](/docs/guides/sites-and-dns/) whose upstream
is the registered service. With a cluster network, Nginx on any edge reaches
the service at its cluster address. Without one, the service listens on
`127.0.0.1` only, so the edge must be the service's own node.

## Limits

- Linux nodes only.
- Images only: there is no `build:`. Images come from registries the node can
  reach.
- Plain Compose per node, no Swarm mode.
- Compose files up to 512 KiB, 100 services, `.env` up to 64 KiB.

## Related

- [Deploy a compose stack](/docs/guides/compose-stacks/)
- [Create a container](/docs/guides/create-container/)
- [Compose extensions and policy](/docs/reference/compose-extensions/)
- [REST API: Compose stacks](/docs/reference/rest-api/#compose-stacks)
