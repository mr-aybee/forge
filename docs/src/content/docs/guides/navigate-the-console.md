---
title: Navigate the console
description: Find pages in the sidebar, pin the items you use most, and jump to any node, service or cluster with the command palette.
sidebar:
  order: 1
---

This guide shows how to move around the web console: the grouped sidebar,
the collapsed sidebar, and the <kbd>Ctrl</kbd>+<kbd>K</kbd> command palette.

## Before you begin

- You are signed in to the console. Any role can use navigation. The sidebar
  and the command palette show only the pages your roles allow; see
  [What each role sees](#what-each-role-sees).
- A modern desktop browser. On narrow windows the sidebar opens from the menu
  button in the header.

## Steps

1. **Find a page in the sidebar.** The sidebar lists pages under short
   headings. A page with siblings sits in a group: click the group, or focus
   it and press <kbd>Enter</kbd>, to show its pages. A group that would hold
   only one page you can open is shown as that page.

   | Heading | Pages |
   | --- | --- |
   | (none) | **Overview** |
   | **Workloads** | **Projects** (the project explorer; see [Browse projects in the explorer](/docs/guides/project-explorer/)) |
   | **Infrastructure** | **Clusters**, **Nodes**, **Network** (**DNS**, **Ingress**, **Terminal**), **Object Storage** |
   | **Operations** | **Alerts**, **Logs**, **Jobs**, **Audit log** |
   | **Packages** | **App Store**, **Installed**, **Updates** |
   | **Admin** | **Approvals**, **Access** (**SFTP access**, **Tunnels**), **Access Control** (**Access**, **Members**), **Credential vault**, **Forge updates**, **Plugins**, **Profile**, **Settings** |

   **Services** and **Stacks** open from the project explorer and the command
   palette. The group that holds the current page opens by itself, and the
   browser remembers which groups you opened.

   Items show live counters from data the console already loads. A counter
   is hidden when it is zero or not loaded yet, and you only see counters for
   pages your roles allow:

   | Item | Counter |
   | --- | --- |
   | **Nodes** | Online nodes out of all nodes (revoked nodes are not counted). The dot is green when all are online, amber while some are pending, red when any is offline |
   | **Alerts** | Firing alerts, in red |
   | **Updates** | Package updates available |
   | **Forge updates** | `1` when a new Forge release is available |
   | **Approvals** | Approvals waiting for you |
   | **Jobs** | Queued and running jobs, with a pulsing dot |
   | **Projects** | Number of projects |

   A closed group shows the sum of its counters, for example pending
   **Updates**. A `new` tag marks recently added pages such as **Logs**.

2. **Use the keyboard.** With focus in the sidebar, <kbd>↑</kbd> and
   <kbd>↓</kbd> move between rows, <kbd>→</kbd> opens a group or enters it,
   <kbd>←</kbd> closes a group or moves to its parent, and <kbd>Home</kbd> and
   <kbd>End</kbd> jump to the first and last row.

3. **Pin what you use most.** Click the pin button in the header of a
   project (in the explorer's details pane), service, node or cluster, or
   choose **Pin to sidebar** in a row's **⋯** menu on the **Nodes**,
   **Clusters** and **Services** lists and in the explorer tree. Pinned
   items appear under **Pinned** at the top of the sidebar, with a status
   dot for nodes. More than six pins of several kinds are grouped into
   **Projects**, **Services**, **Nodes** and **Clusters**.

   To change the order, focus a pinned item and press
   <kbd>Alt</kbd>+<kbd>↑</kbd> or <kbd>Alt</kbd>+<kbd>↓</kbd>, or use
   **Move up** and **Move down** in its **⋯** menu. To unpin, hover the item
   and click **✕**, or press <kbd>Delete</kbd> while it has focus. You can
   pin up to 50 items. Pins are stored per user on the server, and items you
   can no longer see drop out of the list. Pinned items also appear in the
   command palette under **Pinned**.

   :::note
   On a server without pin support the **Pinned** section and the pin
   buttons are hidden.
   :::

4. **Collapse the sidebar.** Click **Collapse** at the bottom of the sidebar.
   It then shows one icon per page or group, with a line between headings.
   An item with a counter shows a dot on its icon; hover it to see the label
   and the count. Click a group icon to open a menu with its pages; nested
   groups such as **Access** open a submenu. The pin icon at the top opens
   your pinned items.

5. **Jump with the command palette.** Press <kbd>Ctrl</kbd>+<kbd>K</kbd>
   (<kbd>⌘</kbd>+<kbd>K</kbd> on macOS), or click **Search** in the sidebar,
   and type. Results are grouped:

   | Group | Matches on | Opens |
   | --- | --- | --- |
   | **Nodes** | Name, hostname, ID, local name such as `<node>.lab.local` | The node page. <kbd>Ctrl</kbd>+<kbd>Enter</kbd> opens a terminal instead |
   | **Services** | Name, ID, project, environment, node names, local service names | The service page |
   | **Clusters** | Name, ID | The cluster page |
   | **Pinned** | Pinned item name | The item's page |
   | **Pages** | Page name; the hint shows its group | The page |
   | **Actions** | Action name | Runs the action |

   Matching is fuzzy: prefixes rank first, then word starts, then any
   substring, then letters in order. Each group shows at most eight results.
   A dot shows status: green online or deployed, amber offline or in
   progress, red failed or revoked, grey unknown or not placed.
   Use <kbd>↑</kbd> and <kbd>↓</kbd> to choose and <kbd>Enter</kbd> to open.

## What each role sees

A group with none of your pages is hidden, and so is a heading left empty. A role on one cluster or one
environment is enough to see the fleet pages; their lists show only what that
role covers. **Overview**, **Approvals** and **Profile** are always there.

| Role (example scope) | Pages beyond the always-shown ones |
| --- | --- |
| `platform_admin` (platform) | Every page |
| `workspace_admin` (workspace) | Fleet pages, **Terminal**, **App Store**, **Updates**, **Alerts**, **Access**, **Members**, **SFTP access**, **Tunnels**. Not **Audit log**, **Credential vault**, **Forge updates**, **Plugins** or **Settings** |
| `operator` (a cluster) | Fleet pages, **Terminal**, **App Store**, **Updates**, **SFTP access**, **Tunnels** |
| `developer` (an environment) | Fleet pages, **Tunnels** |
| `auditor` (workspace) | Fleet pages, **Alerts** |
| `customer` (an environment) | Fleet pages |

Fleet pages are **Clusters**, **Nodes**, **Projects**,
**Services**, **Stacks**, **Installed**, **DNS**, **Ingress**, **Object
Storage** and **Jobs**. **Audit log** needs `audit.read` on the platform, so
an `auditor` bound at platform scope sees it. If you open the address of a
page you can't use, the console shows **You don't have access to this page**.

:::note
Hiding a page is a convenience, not the protection. The server checks every
request against your roles. The full mapping of pages to Cedar actions is in
the [REST reference](/docs/reference/rest-api/#console-capabilities).
:::

## Verify

- The page you open is highlighted in the sidebar and its group is open.
- After you reload the page, the groups you opened are still open.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| A page is missing from the sidebar | Your role does not allow it, or the group is closed | Open the group, or ask an administrator for the role. See [Roles and access](/docs/guides/roles-and-access/) |
| **You don't have access to this page** | You opened a link to a page your roles don't include | Ask an administrator for a role that includes it. A new role shows after you sign in again or reload |
| The palette finds no services | The service list is still loading, or the server does not support it | Wait a moment and type again |
| Local names do not match | Local names are off, or the server is older | See [Local names](/docs/guides/local-names/) |
| Groups do not stay open after a reload | The browser blocks `localStorage` (private mode or a policy) | Allow site storage; navigation works without it |

## Next steps

- [Terminal](/docs/guides/terminal/)
- [Local names](/docs/concepts/local-names/)
