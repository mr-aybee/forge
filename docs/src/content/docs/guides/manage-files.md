---
title: Edit and manage files
description: Edit, create, upload, copy, move, delete, restore and search files on a node from the Files tab.
sidebar:
  order: 16
---

This guide shows you how to change files on a node from the **Files** tab:
edit and save, create files and folders, upload, copy, move, delete and
restore, and search.

## Before you begin

- You need `file.write` (and `file.delete` for moves and deletes) on the
  node, through the **Operator**, **Admin** or **Platform admin** role. For
  the whole-node root you also need `file.node` (**Admin** and
  **Platform admin** only).
- Changes need an MFA-verified session. Set up an authenticator or a passkey
  first; see [Multi-factor authentication](/docs/guides/mfa/). The console asks for a code the first time you
  change something.
- The root you work in must not be read-only. See
  [Approve file roots](/docs/guides/approve-file-roots/).

## Edit a file

1. Open the node (cluster → **Nodes** → the node) and select the **Files**
   tab.
2. Select a text file up to 2 MB. It opens as a preview: highlighted code,
   rendered Markdown or pretty JSON.
3. Select **Edit**, then edit it. Use <kbd>Ctrl</kbd>+<kbd>F</kbd> to find and replace and
   <kbd>Alt</kbd>+click for more cursors. Pick another syntax in the
   language list if the detected one is wrong.
4. Select **Save** or press <kbd>Ctrl</kbd>+<kbd>S</kbd>.

The header shows **Saved**, or **Saved · Validated** for JSON that parsed.
Forge keeps the file's encoding, line endings, owner and permissions.

If someone changed the file since you opened it, Forge does not overwrite
it. The editor shows the file as it is now on the left and your edit on the
right. Merge your changes on the right and select **Save merged version**,
or select **Discard mine and reload**.

## Go back to an earlier version

1. Open the file and select **Revisions** (the clock icon).
2. Select **Load** next to a version. It opens as an unsaved change.
3. Select **Save** to restore it.

Forge keeps the last 10 versions saved from the console, for up to 30 days.

## Create a folder or a file

1. Open the folder that should contain it.
2. Select **New folder** or **New file** and enter the name.
3. For a file, edit it and select **Save**; it is created then.

## Upload files and folders

1. Open the destination folder.
2. Select **Upload**. In the **Upload** dialog, drop files or folders on the
   drop zone, or select **choose files** (or focus the zone and press
   <kbd>Enter</kbd>) to pick files. **Upload a folder…** picks a whole
   folder. You can also drag files and folders straight onto the list; the
   dialog opens with them.
3. Uploads start right away, two files at a time; the rest wait as
   **Queued**. Each row shows the size, a progress bar with the percentage,
   the speed and the time left, then **Verifying** while the node checks the
   SHA-256, and **Done**. Keep adding files while others upload.
4. Select **×** on a row to cancel it, or **↻** to retry one that **Failed**.
   **Clear finished** removes done and cancelled rows.

The listing refreshes as each file finishes. Select **Hide** to close the
dialog while uploads run: they continue, and a chip such as
`2 uploading · 45%` next to **Upload** reopens it. The chip turns red with
`1 failed` when an upload needs attention. Leaving the node page cancels
running uploads, and the browser asks before you close the tab.

Uploads never overwrite an existing file: a name conflict fails that row with
`the destination already exists`. Rename or move the existing file, then
retry. Folder structure is kept. Each upload is checked with SHA-256 before
it appears.

## Rename, copy and move

Right-click a row, or focus it and press <kbd>Shift</kbd>+<kbd>F10</kbd> or the
menu key, to open the same actions as **⋯**. Right-clicking empty space in the
list offers **New folder**, **New file**, **Upload files…**, **Upload folder…**,
**Copy path**, **Show hidden files** and **Refresh**.

1. Select **⋯** at the end of a row, or select the row and press
   <kbd>F2</kbd> to rename.
2. Choose **Rename**, **Copy to…** or **Move to…** and enter the new name or
   the destination path inside the root.
3. To replace an existing destination, select **Replace the destination if
   it exists**.

## Delete and restore

1. Select **⋯** → **Move to trash**, or select the row and press
   <kbd>Delete</kbd>. **Delete permanently** (or <kbd>Shift</kbd>+<kbd>Delete</kbd>)
   skips the trash.
2. To restore, select **Trash**, find the entry and select **Restore**. The
   original folder must still exist.

:::caution
Containers have no trash: deleting there is permanent. Changes inside a
container are lost when the container is recreated.
:::

## Select several items

- Tick the boxes at the start of the rows, or click a row and use
  <kbd>Ctrl</kbd>/<kbd>Cmd</kbd>+click to add rows and <kbd>Shift</kbd>+click
  to select a range. <kbd>Ctrl</kbd>+<kbd>A</kbd> selects everything in the
  folder.
- The selection bar shows how many items and how much data you selected, and
  offers **Download**, **Copy to…**, **Move to…**, **Compress…**,
  **Permissions…**, **Move to trash** and **Delete permanently**. Right-click
  a selected row to get the same actions for the whole selection.
- Several items download as one `.zip`. When some items fail, Forge lists
  each failure and keeps the rest.

## Compress and extract

1. Select the files and folders, then **Compress…** (or **⋯** → **Compress…**
   on one row).
2. Enter the **Archive name** without the extension. The extension of the
   chosen format is shown after the field, and the line below it shows the
   full path the archive gets.
3. Choose the **Format** and select **Compress**, or press <kbd>Enter</kbd>.

   | Node | Formats |
   | --- | --- |
   | Linux | `.tar.gz` (default, smaller, keeps permissions and owners) or `.zip` (opens anywhere) |
   | Windows | `.zip` only; `.tar.gz` is shown disabled |

   **Compress** stays disabled while the name is empty, contains `/`, `\`,
   `< > : " | ? *` or control characters, or matches a file already in the
   folder. The archive is created in the current folder with mode `0644` and
   the folder's owner, like any new file.
4. To unpack, select **⋯** → **Extract…** on an archive, choose a new folder
   and select **Extract**.

:::tip
To update a live site in place, replace its folder instead of extracting to
a new one: call `POST /servers/{id}/files/extract` with `"replace": true`
(needs `file.delete` too). Forge unpacks next to the folder and swaps the new
files in at once; see [Archives](/docs/concepts/file-explorer/#archives) and the
[REST API](/docs/reference/rest-api/).
:::

Forge refuses archives that try to write outside the folder or expand far
beyond their size, and skips links and device files in them; see
[Archives](/docs/concepts/file-explorer/#archives).

## Change permissions and owners (Linux)

1. Select the items, then **Permissions…** (or **⋯** → **Permissions…**).
2. Tick the read, write and run boxes, or type an octal mode such as `0644`.
   Enter an owner and group to change them.
3. Tick **Apply to everything inside folders too** for a whole tree.
4. Select **Preview** to see how many entries change, then **Apply**.

This needs `file.admin` (**Admin** or **Platform admin**) and an MFA check.
On Windows, **Permissions** shows the ACL read-only. On a root on a native
app's folder ([app root](/docs/concepts/file-explorer/#app-roots)), files belong
to the app's account: **Owner** is refused, and setuid and setgid bits are
removed from the mode.

## Search

1. Open the folder to search in and select **Search**.
2. Enter a file name pattern such as `*.conf`, text to find, or both.
3. Choose **Regular expression**, **Match case** or **Whole word** as
   needed, and adjust **Exclude**.
4. Select **Search**. Select a result to open the file.

If the result says **stopped at a limit** or **stopped after 50 s**, select
**Continue** to search further.

## Verify

- The list shows the change after it completes.
- **Operations → Audit log** shows an event such as `file.upload`, `file.move` or
  `file.delete` with the path.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| The code prompt appears on every change | Your MFA check expired | Enter a code; it stays valid for a while |
| `this root is read-only` | The root was approved read-only | Ask an administrator to approve it read-write |
| `source changed` or a merge view on save | The file changed on the node after you opened it | Merge and save, or reload |
| `the destination already exists` | Copy, move or upload would replace a file | Choose another name, or tick **Replace** |
| `another change to this path is in progress` | Someone is saving or moving the same path | Try again in a moment |
| `the original folder no longer exists` | Restore needs the parent folder | Create the folder, then restore |
| `protected path (it contains protected files: …)` on move or delete | The folder holds, or could hold, a protected path (such as `/home/<user>` with `.ssh`) | Move or delete its other contents one by one; see [protected paths](/docs/concepts/file-explorer/#protected-paths) |
| A replaced program lost its setuid or setgid bit | New content never inherits setuid/setgid | An administrator sets the bits again under **Permissions** (`file.admin`) |
| `that archive is gone` on download | The archive expired, was downloaded already, or was made by someone else or through another root | Make the archive again |
| `8.3 short names are not accepted` (Windows) | The path uses a short name such as `PROGRA~3` | Use the long name |
| `too large to copy at once` | Copies are limited to 10,000 entries or 1 GiB | Copy smaller parts |
| `the disk is full` | No space on the node | Free space, then retry; uploads resume |
| `the node is busy with other searches` | Two searches are already running on the node | Try again when they finish |
| `files in an app's folder belong to the app's account; changing the owner is not allowed` | You set **Owner** on an [app root](/docs/concepts/file-explorer/#app-roots) | Leave **Owner** empty. To change an owner anyway, use the whole-node root (`file.node`) |
| `` `.forge` holds the app's launcher, which Forge manages; it cannot be changed here `` | The path is the app's `.forge` folder or inside it | Change the app's command or settings in the service instead; see [Services](/docs/guides/services/) |
| `the app's folder itself cannot be moved or removed here` | You moved or deleted the root's top folder | Move or delete its contents instead, or remove the app |
| `permission denied on …` in an app root | The file is owned by root, typically created by an older Forge, and the app's account cannot change it | Change its owner on the node, for example `sudo chown <environment>-<app>:<environment>-<app> <path>`, or from the whole-node root. Never change `.forge` |
| `<environment>/<app> has no account on this node yet; deploy the app first` | The app is not deployed on this node, or its account was removed | Deploy the app to the node again |
| `/var/lib/forge-app-store/<environment>/<app> is not the app's own folder; remove it and try again` | Something other than the app's own folder sits at the app's trash and revision store | Inspect it, remove it on the node, and retry; Forge creates the store again |
| `the operation did not finish within 120 s` (600 s for a search) | An operation on an app root ran too long | Work on smaller parts, or narrow the search |

## Next steps

- [File explorer](/docs/concepts/file-explorer/): checks, limits and retention.
- [Browse files on a node](/docs/guides/browse-files/)
- [REST API: Files](/docs/reference/rest-api/#files)
- [Command line: `forge nodes files`](/docs/reference/cli/#forge-nodes-files)
