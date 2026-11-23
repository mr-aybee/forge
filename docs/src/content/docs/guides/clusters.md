---
title: Working inside a cluster
description: Create clusters and environments, move environments and nodes between clusters, and set a cluster's badge.
sidebar:
  order: 6
---

This guide shows you how to create a cluster, organise its environments and
move environments and nodes between clusters. Everything that runs in Forge
runs in a cluster. For the model behind it, see [Clusters](/docs/concepts/clusters/).

## Before you begin

- Creating and editing clusters and environments, and moving environments, needs
  `workspace.manage` (**Admin** or **Platform admin**).
- Moving a node needs `server.manage` on the node and `server.enroll` on both
  the old and the new cluster (**Operator** or higher).
- Deleting a cluster needs it to hold no environments.

## Create a cluster

1. Open **Home → Clusters**.
2. Select **Set up a cluster** to create a cluster with members, a network
   and the Nginx ingress in one go, or **Empty cluster** for a cluster with
   only a name.
3. Enter the **Name** and an optional **Description**. In **Set up a
   cluster**, also pick the **Members**; the control node takes part in
   every cluster.
4. Select **Create cluster** (or run the wizard to the end).

Select the cluster's name on **Home → Clusters** to open its page. It has
three tabs:

| Tab | Shows |
| --- | --- |
| **Overview** | The cluster's health (nodes online, services, CPU, memory) and its members |
| **Network** | The cluster network ([Cluster network](/docs/guides/cluster-network/)) |
| **Containers** | Every Docker container on the cluster's nodes |

The **Nodes**, **Services** and **Ingress** cards on **Overview** open the
fleet pages filtered to the cluster (`/nodes?cluster=<cluster-id>`,
`/services?cluster=<cluster-id>`, `/ingress?cluster=<cluster-id>`). On those
pages the **Cluster** filter does the same, and **By cluster** (Nodes,
Ingress) or **By environment** (Services) groups the list.

## Create an environment

An environment is one stage of a project, such as `test`, `staging` or
`production`; it runs in one cluster (see
[Projects, environments and services](/docs/concepts/projects-environments-services/)).
Every environment belongs to a project, so create the project first under
**Projects** if it does not exist.

1. Open **Projects** and select **New environment**, or open the project's
   row menu and select **New environment…**.
2. Pick the **Project** (or create one inline with **New project**), enter
   the **Name**, pick the **Cluster** and tick **Protected** for a
   production-like stage.
3. Select **Create**. The change is audited as `environment.create`.

**Services**, filtered to the cluster in the **By environment** layout,
lists the cluster's environments with their project and services.

## Move an environment to another cluster

1. Open **Projects** and select the environment in the tree.
2. Open its row menu (or right-click its row) and select **Move to
   cluster…**. The menu item is disabled when there is no other cluster.
3. Pick the **Target cluster**, read **What happens** and confirm.

The environment moves with its services. You need `workspace.manage` on
both the environment and the target cluster. The move is audited as
`environment.move`. An environment always stays in a cluster.

To move an environment to another project, select **Move to project…** in
the same menu (audited as `environment.set_project`).

:::note
A role on a cluster covers the cluster's nodes, not the environments placed in
it. Moving an environment therefore does not change who can deploy to it. See
[Authorization](/docs/concepts/authorization/#cluster-scope-and-environment-scope).
:::

## Move a node to another cluster

1. Open the node and select the **Details** tab.
2. Under **Placement**, select **Move to cluster…**.
3. Check the dialog: it shows the move as *from → to*, what happens, and
   which services in the old cluster run on the node.
4. Select **Move node**.

Forge then hands the node over:

1. The node leaves the old cluster's network. Forge removes that cluster's
   network interface from the node in the background.
2. The node joins the new cluster's network and gets an address there.
3. Who can manage the node follows its cluster.

Services in the old cluster that pointed at the node must be moved or
updated.

:::caution
Forge refuses the move while the node is the old cluster's ingress edge.
The dialog shows which role blocks it. Move that role to another node first
on **Ingress** with the old cluster selected.
:::

## Set the cluster badge

1. Open **Home → Clusters** and select **Edit cluster** on the cluster.
2. Under **Badge**, upload a PNG, JPEG, WebP or SVG image (up to 128 KB;
   raster images are scaled to 128 px), or set up to four letters.
3. Select **Save changes**.

Without an image or letters, the initials of the name are used (`lab` shows
**LA**, `edge-eu` shows **EE**). The badge appears next to the cluster name
in the **Clusters** list.

## Delete a cluster

Select **Delete cluster** on **Home → Clusters**. A cluster that still has
environments cannot be deleted: the dialog lists them and stays disabled until
you move them.

## Verify

- The cluster appears on **Home → Clusters**.
- A moved environment shows its new cluster in its pane on **Projects**.
- A moved node is listed on **Nodes** filtered to the new cluster and gets an
  address on the cluster's **Network** tab.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `409` when moving an environment | The target cluster already has an environment with that name | Rename one of them with **Rename…** in its row menu on **Projects** |
| **Move node** is refused with an edge message | The node is the old cluster's ingress edge | Move that role to another node first |
| `403` when moving a node | You lack `server.enroll` on the old or new cluster | Ask for **Operator** on both clusters |
| **Delete cluster** stays disabled | The cluster still has environments | Move them with **Move to cluster…** |
| A cluster operator cannot deploy to an environment in it | Cluster bindings do not cover environments | Bind the role on the environment, its project or the workspace |

## Next steps

- [Cluster network](/docs/guides/cluster-network/)
- [Ingress](/docs/guides/ingress/)
- [Roles and access](/docs/guides/roles-and-access/)
- [Add nodes](/docs/guides/add-nodes/)
