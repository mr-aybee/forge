---
title: Browse projects in the explorer
description: Find any service through its project and environment, manage projects and environments, see where services run and how healthy they are, and act on one node without leaving the tree.
sidebar:
  order: 5
---

This guide shows you how to use the **Projects** page, the project explorer.
It shows every project, its environments and their services as one tree,
with the nodes each service runs on. You can create and change projects and
environments, find a service, read its logs on one node and restart it there
without leaving the page. For the model behind the tree, see
[Projects, environments and services](/docs/concepts/projects-environments-services/).

## Before you begin

| Task | Needs |
| --- | --- |
| See the tree | `view` on each environment (**Viewer** or higher); environments you cannot view are left out |
| See health-check status | `health.view` on the service |
| Read logs | `view` on the environment |
| Start, restart or stop | `deploy` on the environment (**Developer**, **Operator**, **Admin** or **Platform admin**) |
| Create, edit or delete a project | `workspace.manage` on the workspace or the project (**Admin** or **Platform admin**) |
| Create, rename, move or delete an environment | `workspace.manage` on the project, the environment and the cluster involved (**Admin** or **Platform admin**) |

- The explorer works in any browser the console supports. Below the `lg`
  width (about 1024 px) it shows the tree first and the details after you
  pick an item.
- Services of a compose stack appear in the tree, but you act on them from
  their stack.

## Steps

1. **Open the explorer.** Select **Projects** in the sidebar. The header has
   **New project** and **New environment**; **New environment** preselects
   the project you have selected.

2. **Expand the tree.** Click the arrow next to a project or an
   environment, or use the keyboard:

   | Key | Action |
   | --- | --- |
   | <kbd>↑</kbd> / <kbd>↓</kbd> | Previous / next row |
   | <kbd>→</kbd> | Expand, or go to the first child |
   | <kbd>←</kbd> | Collapse, or go to the parent |
   | <kbd>Home</kbd> / <kbd>End</kbd> | First / last row |
   | <kbd>Enter</kbd> or <kbd>Space</kbd> | Select the row |
   | A letter | Next row whose name starts with it |

   **Expand all** and **Collapse all** sit in the panel header. Environments
   without a project appear under **Unassigned**; open one's row menu and
   select **Attach to project…** to place it.

3. **Search and filter.** Type in the search box. The search matches
   project, environment, service, runtime and node names. A matching service
   stays under its environment and project, and the path to it opens. A
   matching project or environment shows everything in it. Narrow the tree
   with the filters in the filter bar:

   | Filter | Shows |
   | --- | --- |
   | **Health** | **Unhealthy**: services that fail or are degraded, or whose last deploy failed. **Deploying**: services with a deploy in progress |
   | **Runtime** | Services of one runtime (`systemd`, `container`, or a stack runtime) |
   | **Cluster** | Environments in one cluster (shown when you have two or more clusters) |

   The search, the filters and the selection are kept in the page address
   (`/projects?q=…&cluster=…&runtime=…&health=unhealthy&sel=service:<service-id>`),
   so a filtered view is a link you can share.

4. **Read a service row.** Each service appears once in its environment. The
   row shows:
   - a dot for whether it runs: green **Running**, amber **Degraded**, pale
     amber **Stopped**, red **Failed**, grey **Unknown**. A service you
     stopped shows **Stopped**, not **Failed**, even though its health checks
     fail. While it runs, the dot follows the health checks of its active
     nodes; where a node has no check result yet, it uses the state the node
     last reported;
   - a **deploying** or **deploy failed** badge while a rollout is in
     progress or after it failed. Rollout state is shown apart from the dot:
     a service can run its previous version while a deploy fails;
   - one tag per node it is placed on. Standby nodes have a dashed border.
     Hover a tag for its role, deploy status and last error; click it to open
     the node. After three tags, **+N** shows the rest.

   Projects and environments show the worst dot of what they hold.

5. **Open the details.** Click a row. The right side shows the item's
   pane. The selection is kept in the page address (`?sel=project:<id>`,
   `env:<id>` or `service:<id>`), so you can reload the page or share the
   link.

   - A **project** pane shows its description, id, health, service count,
     creation time and its environments (each with its cluster and service
     count), with the buttons **New environment…**, **Edit…** and
     **Delete…**.
   - An **environment** pane shows its id, cluster, protection, health,
     creation time, services and a tags editor,
     with the buttons **New service here**, **Rename…**, **Move to
     project…**, **Move to cluster…** and **Delete…**.

    For a service, the **Placements** list
   has one row per node with the same signals as the service page's **Nodes**
   table: role (**replica** or **standby**), deploy result, state the node
   reported (**running**, **stopped**, **failed**, …) and, while it runs, the
   health-check status. After you start, stop or restart, the row updates
   within about ten seconds, once the node reports; **Refresh** in the panel
   header reloads it at once. Each row has buttons for
   that node; on hover they turn green (start), amber (restart), red (stop)
   or blue (logs):

   | Button | Does |
   | --- | --- |
   | **Logs from `<node-name>`** | Opens the service's log on that node in the console dock |
   | **Start on `<node-name>`** | Starts the service on that node. Disabled while it already runs there |
   | **Restart on `<node-name>`** | Restarts it on that node, after you confirm |
   | **Stop on `<node-name>`** | Stops it on that node, after you confirm. Other nodes keep serving. Disabled while it is already stopped there |

   **Logs** above the list opens a side panel where you pick one node or
   **All nodes**. **All nodes** follows up to eight nodes at once and merges
   their lines by time, each prefixed with its node name. It keeps the newest
   5000 lines and has pause, filter, copy and clear. **Open service** opens
   the full service page, where you deploy a changed definition.

6. **Use the row menu.** Hover a row, focus it with the keyboard or
   right-click it for its menu:

   | Row | Actions |
   | --- | --- |
   | Project | **New environment…**, **Edit…**, **Delete…** (disabled while it has environments) |
   | Environment | **New service here** (disabled without a cluster), **Rename…**, **Move to project…** (**Attach to project…** when it has none), **Move to cluster…** (disabled when there is no other cluster), **Delete…** (disabled while it has services) |
   | Service | **Logs**, **View**, **Edit configuration** |

   Each action opens a dialog: **New project**, **Edit "<name>"**, **Delete
   project "<name>"?**, **New environment** (with **Project**, **Name**,
   **Cluster**, **Protected** and an inline **New project**), **Rename
   "<environment>"**, **Move "<environment>" to another project**, **Move
   "<environment>" to a cluster** (with a **What happens** list) and **Delete
   environment "<environment>"?**. Changes are audited as `project.*` and
   `environment.*`.

:::note
Disabled buttons say why when you hover them: for example, you lack `deploy`
on the environment, the node has no deployment yet, or another action is
running. The console hides nothing for security: the server authorizes every
action again, and an action that needs MFA asks for your code.
:::

## Verify

- Search for the name of a node that runs a service. The service shows under
  its environment and project, with the path open.
- Select a service placed on two nodes. It appears once in the tree, with two
  node tags, and **Placements** lists both nodes.
- Click **Logs from `<node-name>`** on one placement. The dock opens a tab
  named after the service and that node.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| An environment is missing | You lack `view` on it | Ask an admin for a role on the environment or its project |
| **Health checks could not be loaded** above the tree | The health summary request failed, or you lack `health.view` | Dots fall back to the state nodes last reported. Reload; check your role |
| **Health checks cover the first 500 services** | More than 500 services are visible | Dots of the rest use the node-reported state; open a service's **Health** tab for its checks |
| Start, Restart and Stop are disabled | No `deploy` on the environment, a stack service, a node not deployed yet, or a job in progress | Hover the button for the reason |
| **Delete…** is disabled on an environment | It still has services | Delete or move its services first |
| **All nodes** shows an error badge for a node | That node's agent is offline or the log read failed | Pick the node alone to see the error, or check the node's page |

## Next steps

- [Services](/docs/guides/services/): create, place and deploy services.
- [Health checks](/docs/guides/health-checks/): configure what the dots measure.
- [REST API](/docs/reference/rest-api/): `GET /services/health-summary` and
  `can_deploy`.
