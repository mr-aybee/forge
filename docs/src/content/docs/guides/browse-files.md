---
title: Browse files on a node
description: Browse directories, view text files and download files on a Linux or Windows node from the console.
sidebar:
  order: 15
---

This guide shows you how to browse a node's filesystem, view a file and download
it from the **Files** tab, without opening a terminal.

## Before you begin

- To browse the whole node you need `file.read` and `file.node` on it: the
  **Platform admin** and **Admin** roles. MFA is not required to read.
- Operators have `file.read` only: they see the service and container
  directories an administrator approved. See
  [Approve file roots](/docs/guides/approve-file-roots/).
- The node runs Debian/Ubuntu, a RHEL-like distribution or Windows, with an
  agent that allows the `files` command (the default).
- The node is online.

:::caution
The `node` root covers the node's whole filesystem except
[protected paths](/docs/concepts/file-explorer/#protected-paths).
:::

## Steps

1. Open the node (cluster → **Nodes** → the node).
2. Select the **Files** tab. If you can open more than one root, choose it
   in the list at the top left. The node root opens at `/` on Linux and
   lists the drives on Windows.
3. Select a folder to open it. Use the breadcrumbs or **Up one level** to go
   back.
4. To jump to a path, type it in the path box and press <kbd>Enter</kbd>, for
   example `/var/log` or `/C:/inetpub/logs`.
5. Select **Show hidden files** to include dotfiles and Windows hidden files.
6. Select a file to view it. Text files up to 2 MB open in the editor
   (read-only if you can't change the root); text up to 50 MB can be paged
   with **Load more**; images show a preview.
7. Select the download icon on a row or in the viewer to download the file.

## Verify

- The breadcrumbs show the path you opened, and the footer shows the number of
  entries.
- **Operations → Audit log** shows a `file.list` event for each directory you opened and a
  `file.read` or `file.download` event for each file.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| The **Files** tab shows **No file roots** | You have no root: not `file.node`, and no directory is approved for you | Ask an administrator to approve a directory, or for the **Admin** role |
| An error says `forbidden` | You lack `file.read` on the node | Ask for the **Operator** role on the node, its cluster or workspace |
| `the container is not running` | A container root needs its container running | Start the container |
| `the container was replaced` | The container was recreated; roots are bound to its id | Approve the new container's directory |
| `protected path (…)` | The path holds keys, credentials or virtual files | This is by design; see [protected paths](/docs/concepts/file-explorer/#protected-paths) |
| `… is a link …; Forge does not follow links` | The path goes through a symbolic link, junction or mount point | Open the link's target path directly; the listing shows it after `→` |
| `the node did not answer in time` | The node is offline, or its agent is older than the control plane | Check the node's status; [update the agent](/docs/guides/update-agents/) |
| `this node's agent does not support the file explorer` | `files` is missing from `allowed_commands` in the agent's `agent.toml` | Add `"files"` and restart the agent, or re-run the node installer |
| A download stops with an error | The file changed while it downloaded | Download it again |
| `files over 512 MiB cannot be downloaded through the console; use SFTP access` | The console relays at most 512 MiB | Use [temporary SFTP access](/docs/guides/sftp-access/) |

## Next steps

- [File explorer](/docs/concepts/file-explorer/): checks, confinement and limits.
- [Edit and manage files](/docs/guides/manage-files/): save, upload, move, delete and search.
- [REST API: Files](/docs/reference/rest-api/#files): the endpoints behind this tab.
- [Roles and access](/docs/guides/roles-and-access/): grant **Operator** on a node.
