---
title: Approve file roots
description: Give operators file access to one service or container directory instead of the whole node.
sidebar:
  order: 17
---

This guide shows you how to approve a service or container directory as a
file root, so operators can browse and, if you allow it, change files there
without access to the rest of the node.

## Before you begin

- You need `file.admin` on the node: the **Platform admin** or **Admin**
  role, with an MFA-verified session.
- The directory must exist and must not be a
  [protected path](/docs/concepts/file-explorer/#protected-paths).
- Container roots need a Linux node with Docker and a running container. You
  need its full 64-character id: run `docker inspect --format '{{.Id}}' <container-name>`
  on the node.
- For a native app's folder on Linux you usually need no approval: Forge
  creates a writable app root as soon as the app is placed on the node. See
  [Services](/docs/guides/services/). Approve one by hand only if you
  removed it, or for a read-only root.
- A node has one root per kind and folder. Approving a folder that is
  already a root fails with `409`; remove the existing root first to change
  it.

## Steps

1. Open the node (cluster → **Nodes** → the node) and select the **Files**
   tab.
2. Select **Roots**.
3. Choose **Service directory** or **Container directory**.
4. Enter a **Label**, such as `web app`.
5. For a container, enter the **Container id**.
6. Enter the **Directory**, such as `/srv/app` on Linux or `/D:/sites/app`
   on Windows, or the **Path inside the container**, such as `/usr/share/nginx/html`.
   For a native app on Linux, enter the app's folder itself:
   `/srv/forge/apps/<environment>/<app>`.
7. Leave **Read-only** selected unless operators should change files there.
8. Select **Approve root**.

:::caution
A read-write root lets operators create, change and delete files in it.
Approve the narrowest directory that does the job; a broad one such as a
working directory of `/` gives them most of the node.
:::

### Roots on a native app's folder (Linux)

A root on exactly `/srv/forge/apps/<environment>/<app>` is an
[app root](/docs/concepts/file-explorer/#app-roots). Every operation on it runs on
the node as the app's account (`<environment>-<app>`), not as root, and so does
[SFTP access](/docs/guides/sftp-access/) to it. Roots you approved on an app's
folder earlier switch to this on their own.

What changes for people who use the root:

- New files and uploads belong to the app's account.
- **Owner** in **Permissions** is refused. Permission bits work, but setuid
  and setgid are removed.
- The `.forge` folder (the app's launcher, owned by root) cannot be opened,
  changed, renamed or deleted, and the app's folder itself cannot be moved or
  deleted.
- Files that root created in the folder earlier stay owned by root, and are
  read-only through the explorer.
- The trash and revisions are kept per app and removed when you remove the
  app.

Forge refuses a root on a folder that holds app folders (`/srv`,
`/srv/forge`, `/srv/forge/apps`, `/srv/forge/apps/<environment>`) or on a
folder inside an app's folder, such as
`/srv/forge/apps/<environment>/<app>/public`. Approve the app's folder itself.
The permissions are the same as for any root: `file.read`, `file.write`,
`file.delete` and `file.admin` on the node.

## Verify

- The root appears in the **Roots** dialog and in the root list at the top
  of the **Files** tab.
- An operator on the node sees the root and nothing else.
- **Operations → Audit log** shows `file.root.create` with the root. For an
  app root, its `spec` is `{"kind":"app","environment":"<environment>","app":"<app>"}`,
  and later file events on it carry `app` and `as_reported`, the account
  the node used.
- For an app root, upload a test file and check its owner on the node:

  ```sh
  ls -l /srv/forge/apps/<environment>/<app>/<file>
  ```

  The owner is the app's account, such as `<environment>-<app>`.

To remove a root, open **Roots** and select the bin icon next to it. Files
are not touched.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `step_up_required` | Approving needs MFA | Enter a code when asked |
| `protected path (…) cannot be a root` | The directory holds keys, credentials or virtual files | Choose another directory |
| `the root must be a directory` or `was not found` | The path does not exist, or is a file or link | Check the path on the node |
| `a container root needs the full 64-character container id` | A short id or a name was given | Use the full id from `docker inspect` |
| `container roots need a Linux node` | Container roots are not supported on Windows | Use a service directory |
| The root stops working after a redeploy | The container was recreated with a new id | Approve the new container's directory |
| `` `/srv/forge/apps` holds native apps' folders; approve one app's folder (/srv/forge/apps/<environment>/<app>) instead `` | The directory is `/srv`, `/srv/forge`, `/srv/forge/apps` or an environment folder, which hold several apps' folders | Approve one app's folder |
| `` `…` is inside a native app's folder; approve the app's folder itself (/srv/forge/apps/<environment>/<app>) `` | The directory is below an app's folder, such as its `public` folder | Approve the app's folder named in the message. A root approved earlier on such a folder shows the same message when used: remove it and approve the app's folder |
| `<environment>/<app> has no account on this node yet; deploy the app first` | The app is not placed on this node, its prepare job failed, or its account was removed by hand | Place the app on this node, or run the `prepare` action, then approve the root again |
| `more than one account claims to be <environment>-<app>'s (…)` | An account Forge did not create imitates the app's account | Remove the account Forge did not create, on the node |
| `app roots need a Linux node` | App roots exist only on Linux nodes | Use a service directory on the Windows node |
| `` this folder is already a root on this node (`fr_…`, …) `` | The folder is approved already, by hand or automatically for its app | Use the existing root, or remove it first to approve it with other settings |

## Next steps

- [File explorer](/docs/concepts/file-explorer/#roots): what roots cover.
- [App roots](/docs/concepts/file-explorer/#app-roots): how a root on an app's
  folder runs as the app's account.
- [Edit and manage files](/docs/guides/manage-files/): what you can change in a
  root.
- [Roles and access](/docs/guides/roles-and-access/): grant **Operator** on a node.
