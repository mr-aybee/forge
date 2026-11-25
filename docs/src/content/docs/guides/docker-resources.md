---
title: Docker images, volumes and networks
description: Pull, tag, prune and remove Docker images, volumes and networks on a node, and read its Docker events and disk use.
sidebar:
  order: 6
---

This guide shows you how to manage the Docker objects of one node from its
**Docker** tab: images, volumes, networks, recent daemon events and disk use.
For containers and stacks, see [Containers](/docs/guides/containers/).

## Before you begin

- The node runs Linux with **Docker Engine** installed from the
  [App Store](/docs/guides/app-store/), and its agent is online. Windows nodes
  are not supported.
- The Docker TLS endpoint is not needed; the node's root helper runs each
  operation.

| Action | Needs |
| --- | --- |
| See images, volumes, networks, events and disk use; inspect any of them | `server.manage` on the node |
| Pull, push, tag, remove and prune images | `docker.image.manage` |
| Create, remove and prune volumes | `docker.volume.manage` |
| Create, remove, connect, disconnect and prune networks | `docker.network.manage` |

**Operator**, **Admin** and **Platform admin** hold all of these.

## Steps

1. Open the node and select the **Docker** tab. Pick a sub-tab:
   **Containers**, **Images**, **Volumes**, **Networks**, **Events** or
   **Disk**. The sub-tab is part of the page address, so you can link to it.
2. **Images.** Filter by tag, id or digest, and narrow to **Dangling**
   (untagged layers) or **Unused** (no container uses it).
   - **Pull image** takes a reference such as `nginx:1.27` or
     `ghcr.io/org/app:1.2`. The pull runs as a job; the toast's
     **Follow job** opens its live log.
   - The row menu offers **Inspect**, **History**, **Tag…**, **Push…** and
     **Remove…**. **Inspect** shows tags, digests, platform, entrypoint and
     command, ports, volumes, environment, labels and layers; switch to
     **Raw JSON** for the full document, with **Copy JSON**. Environment
     values show **hidden** unless you have `container.inspect_env`.
     **Push…** works only for a tag whose host has a registry in
     **Settings** → **Registries**. **Remove** refuses an
     image a container uses unless you turn on **Force**.
   - Select rows with the check boxes and use **Remove** for several unused
     images at once. Images in use cannot be selected.
   - **Prune** removes dangling images; turn on **Also remove unused tagged
     images** to remove every image no container uses.
3. **Volumes.** **Create volume** takes a name and optional `key=value`
   labels. **Remove…** deletes the volume and its data; **Force** removes it
   even while a container uses it. **Prune** deletes anonymous volumes no
   container uses. Volumes Forge created carry a **Forge** badge and cannot be
   selected for bulk removal.
4. **Networks.** **Create network** takes a name, a driver (`bridge` or
   `overlay`), an optional subnet and gateway, **Internal** and, for overlay
   networks, **Attachable**. The row menu offers **Inspect**, **Connect
   container…**, **Disconnect container…** and **Remove…**. **Prune** removes
   custom networks with no containers attached.
   :::caution[Prunes are host-wide]
   Every prune acts on the whole node, not only on what Forge created: it
   removes every unused resource of its kind on that host, including those of
   containers and tools Forge does not manage. Forge's own networks are kept;
   named volumes are never pruned. The confirmation dialog says so.
   :::
5. **Events.** Choose **5m**, **1h** or **24h**. Events are listed newest
   first and refresh every 15 seconds; **Refresh** reads them now.
6. **Disk.** Four tiles show the count and size of images, container
   writable layers, volumes and build cache, with shortcuts to prune dangling
   images and anonymous volumes.

:::caution
Removing or pruning volumes deletes their data for good. Removing an image a
running container uses (with **Force**) leaves that container running, but it
cannot be recreated until the image is pulled again.
:::

:::note
Docker's own networks (`bridge`, `host`, `none`, `ingress`,
`docker_gwbridge`) and Forge's networks (`forge-*`, or labelled
`forge.managed=true`) show a **protected** badge. They cannot be removed,
connected or disconnected here, and `forge.*` labels are reserved.
:::

## Verify

- After a create or remove, the list refreshes and shows the change.
- After a pull, push or prune, the job in **Operations → Jobs** shows
  `succeeded`, and the **Disk** tab shows the new totals.
- The **Events** tab lists the operation, for example `image pull` or
  `volume destroy`.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| **Docker Engine is not installed or not reachable** | Docker is missing or its service is stopped | Install **Docker Engine** from the App Store, or start the `docker` service |
| "This Forge server does not offer Docker images, volumes and networks yet" | The server is older than the console | Update Forge |
| An action fails with `403` | You lack the `docker.*.manage` action for it | Ask for **Operator** on the node or its cluster |
| An action fails with `400` and a protected network or reserved label | The network belongs to Docker or Forge, or a label starts with `forge.` | Use another network or label |
| Removing an image or volume fails with "in use" | A container uses it | Remove the container first, or turn on **Force** |
| Creating an overlay network fails | The node is not a Swarm manager | Create it on a manager; see [Docker Swarm](/docs/guides/docker-swarm/) |

## Next steps

- [Containers](/docs/guides/containers/)
- [Docker Swarm](/docs/guides/docker-swarm/)
- [REST API: Docker images, volumes and networks](/docs/reference/rest-api/#docker-images-volumes-and-networks)
