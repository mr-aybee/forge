---
title: File explorer
description: How Forge reads and changes files on a node, what each root covers, which paths it refuses, why an operation cannot escape its root and how app roots run as the app.
sidebar:
  order: 9
---

The file explorer lets you browse, view, edit, upload, copy, move and delete
files on a node from the console, without opening a terminal. This page
explains where each check happens and what the explorer can reach. To use
it, see [Browse files on a node](/docs/guides/browse-files/) and
[Edit and manage files](/docs/guides/manage-files/).

## Roots

Every request names a **root** and a **path** inside it. A root is the
directory a user works in; the path can never leave it.

| Root | Covers | Who can open it |
| --- | --- | --- |
| `node` | The whole filesystem: `/` on Linux, every local drive on Windows (`/C:/Users`) | `file.read` and `file.node`: `platform_admin`, `workspace_admin` |
| Service | An approved directory on the node, such as `/srv/app` or `/D:/sites/app` | `file.read`: also `operator` |
| Container | An approved directory inside a running Docker container (Linux) | `file.read`: also `operator` |

An administrator approves service and container roots in the
explorer's **Roots** dialog (`file.admin`, MFA). A root can be read-only,
which is the default, or read-write. A read-only root refuses every change,
whatever the user's roles. See [Approve file roots](/docs/guides/approve-file-roots/).

On Linux, a service root on a native app's folder,
`/srv/forge/apps/<environment>/<app>`, is an [app root](#app-roots): its
operations run as the app's own account, not as root.

Paths are slash-separated and start with `/`. Forge rejects `.` and `..`
components, NUL bytes, components over 255 bytes, paths over 4096 bytes and
paths deeper than 128 levels, in the API and again on the node.

:::caution
The `node` root covers the whole filesystem except protected paths: anyone
who can open it reads and, with `file.write`, changes what the node's root
user or LocalSystem can. Only `platform_admin` and `workspace_admin` have
`file.node`. Give operators approved directories instead.
:::

## Permissions

| Operation | Cedar actions on the server |
| --- | --- |
| List, view, download, hash, search, read revisions and the trash | `file.read` |
| Create folders, save, upload | `file.read`, `file.write` |
| Copy | `file.read`, `file.write` |
| Move, rename | `file.read`, `file.write`, `file.delete` |
| Delete, empty the trash | `file.read`, `file.delete` |
| Restore from the trash | `file.read`, `file.write` |
| Approve or remove roots | `file.admin` |

The `node` root also needs `file.node`. `file.write`, `file.delete` and
`file.admin` need an MFA-verified session: the console asks for a code the
first time you change something.

## Where checks happen

```diagram
alt: File explorer request path and checks
flow:
  steps:
    - { label: Browser, edge: "" }
    - { label: API, edge: "" }
    - { label: FileService, note: "roots, Cedar, read-only roots, protected paths, upload ownership, audit", tone: accent, edge: "" }
    - { label: signed command, edge: "" }
    - { label: agent, note: "allowlist, path rules, protected paths, confinement", edge: "" }
    - { label: filesystem }
```

1. The API hands every request to the file service, the same service the
   SFTP gateway uses.
2. The service resolves the root (the browser sends only its id), checks
   every Cedar action the operation needs, refuses changes to read-only
   roots and applies the protected-path policy to every path the operation
   touches. Refusals are audited.
3. The service sends one signed `files` command over the agent's mTLS link.
   No job is created.
4. The agent checks its local allowlist, validates the operation again and
   applies the protected-path policy again.
5. The agent resolves every path without following links, as described
   below, and runs the operation in the background so heartbeats and other
   commands keep flowing.

| OS | Who touches the files |
| --- | --- |
| Debian/Ubuntu, RHEL-likes | The root helper (`forge-agent-helper`). The agent forwards the operation over the helper's Unix socket; the helper runs every check again before it opens anything. On an [app root](#app-roots) the helper starts a worker process as the app's account, which does the work |
| Windows | The agent service (LocalSystem), in process |

Files Forge creates take the owner and group of their folder (Linux). On an
app root they belong to the app's account.
Replaced files keep their owner and permission bits, except setuid and
setgid: new content never inherits them, so uploading over a setuid program
does not make your bytes privileged. An administrator can set them again
under **Permissions** (`file.admin`). ACLs and extended attributes are not
copied.

## Confinement

**Linux** opens a root's base directory, then resolves each path one
component at a time, each relative to the directory opened before it, with
`O_NOFOLLOW`. Creating, renaming and deleting use the parent's handle
(`mkdirat`, `renameat2`, `unlinkat`). A symbolic link is never followed:
listing shows its target, opening it fails, deleting it removes the link and
not its target. A link swapped in while an operation runs fails the same
way, so there is no gap between checking a path and using it. Because no
link is followed, the written path is the real path, which is what the
protected-path policy matches. When a file with several hard links is opened,
its inode is compared with the files in the protected locations at that
moment.

**Windows** refuses symbolic links, junctions and mount points (reparse
points) on the way to a path, opens files with `FILE_FLAG_OPEN_REPARSE_POINT`
and checks the open handle again. Names with `:` (alternate data streams),
reserved device names such as `CON` or `LPT1`, names ending in a dot or
space, and 8.3 short names such as `PROGRA~3` are rejected. A short name is
another spelling of a long one, so accepting it would let a path such as
`C:\PROGRA~3\Forge` past the protected-path names. Use the long name.

:::note
On Windows the folders on the way are checked by path before the file is
opened, so a folder swapped for a junction during that window is not
detected. The final file is checked on the open handle. Hard links to
protected files are not detected on Windows.
:::

**Containers** are reached through the container's own root directory
(`/proc/<pid>/root` of its init process) on the node, with the same
component-by-component, no-follow resolution, so shell-less images work. A
root is bound to the full container id: if the container is recreated, its
roots stop working. The container must be running. Changes live in the
container's writable layer and are lost when it is recreated; the explorer
says so. Containers have no trash or revisions.

## Protected paths

Forge never lists, reads, writes, copies or deletes these paths, whatever
the user's roles. They appear in listings with a lock.

| OS | Protected |
| --- | --- |
| Linux | `/proc`, `/sys`, `/dev`, `/run`, `/etc/forge` (including package credentials in `/etc/forge/pkgs`), `/var/lib/forge`, `/var/lib/forge-agent`, `/var/lib/forge-app-store` (the trash and revisions of app roots), `/root/.ssh`, `/home/*/.ssh`, `/etc/shadow`, `/etc/gshadow` and their backups, `/etc/ssh/ssh_host_*_key` |
| Windows | `C:\ProgramData\OrbynForge` (the agent's data: key, identity, WireGuard keys, package settings and file store), `C:\ProgramData\Forge`, `C:\Windows\System32\config`, `pagefile.sys`, `hiberfil.sys` and `swapfile.sys` at a drive root, `C:\Users\*\.ssh` |
| Containers | The Linux list, inside the container |

Comparisons on Windows are case-insensitive. A copy or search that reaches a
protected path skips it or stops there.

A move or delete takes everything below a folder along, so Forge also
refuses to move, delete or replace a folder that holds a protected path or
could hold one. The same applies to the destination of a move. Move or
delete the folder's other contents one by one instead.

| OS | Never moved or deleted as a whole |
| --- | --- |
| Linux | `/home`, `/home/<user>`, `/root`, `/etc`, `/etc/ssh`, `/var`, `/var/lib`, and every other parent of a protected path |
| Windows | A drive, `C:\ProgramData`, `C:\Windows`, `C:\Windows\System32`, `C:\Users`, `C:\Users\<user>` |

## App roots

Each native app on a Linux node runs as its own system account,
`<environment>-<app>`, with its folder `/srv/forge/apps/<environment>/<app>`
(`0750`, owned by that account); see
[Services](/docs/guides/services/#layout-and-isolation). When you approve
exactly that folder as a root, it becomes an **app root**: every operation
on it runs on the node as the app's account, not as root. That covers
listing, reading, saving, uploads, downloads, renames, copies, deletes and
the trash, search, archives and `chmod`. Roots approved on an app's folder
before this existed run as the app the next time they are used.

The reason is containment. Forge's path checks keep each operation inside
its root, but a bug in those checks would let a root-run operation reach the
whole node. An app root's operations run with the app's rights, so the same
bug reaches no more than the app's own account could: other apps' folders
are `0750` and owned by their own accounts, and `/etc/forge` is not readable
to it.

Forge refuses roots that would run as root over an app's files:

| Directory | Why it is refused |
| --- | --- |
| `/srv`, `/srv/forge`, `/srv/forge/apps`, `/srv/forge/apps/<environment>` | It holds app folders, possibly of several apps with different accounts |
| A folder inside an app's folder, such as `/srv/forge/apps/<environment>/<app>/public` | It belongs to one app; approve the app's folder itself |

The whole-node root (`file.node`) still covers these folders and runs as
root, as before.

On an app root, some things work differently from a plain directory root:

| | On an app root |
| --- | --- |
| Owner of new files and uploads | The app's account |
| Changing owners (**Owner**, `chown`) | Refused |
| `chmod` | Allowed; setuid and setgid bits are removed |
| `.forge` (the app's root-owned launcher folder) | Listed, but cannot be opened, changed, renamed or deleted |
| The app's folder itself | Cannot be moved or deleted |
| Symbolic links | Listed as links and never followed by reads, writes, copies or extraction; renaming or deleting the link itself works |
| Files root created in the folder earlier | Stay owned by root and are read-only through the explorer. Forge does not change their owner |
| Trash and revisions | In `/var/lib/forge-app-store/<environment>/<app>/`, removed when the app is removed |
| Permissions needed | Unchanged: `file.read`, `file.write`, `file.delete`, `file.admin` on the server, with MFA as for any root |

The trash and revisions live outside the app's folder because apps often
serve that folder to the web. The store folder is `0700` and owned by the
app's account; its parents `/var/lib/forge-app-store` and
`/var/lib/forge-app-store/<environment>` are `0711` and owned by root, and the
whole tree is a [protected path](#protected-paths). Each app has its own
folder per environment, so apps never share a store.

An app root needs the app's account on that node. Forge creates the
account, the folder and the root itself: when you place a native app on a
Linux node, an `app.action` job **prepares** it there (account, folder,
store; nothing starts), and Forge then approves a writable app root labelled
with the app's name and linked to the service (`application_id`). A
deploy does the same for apps placed before this existed. When the app is
removed from the node, or deleted, its root goes too; the files stay. If a
root on that folder exists already, approved by hand, Forge keeps it as it
is and does not add a second one. These automatic roots are audited like
manual ones (`file.root.create`, `file.root.delete`) with `automatic: true`.

Forge is the one that hands the root out because placing or deploying an app
already needs `deploy` on the environment and `server.manage` on the node, and
the root covers nothing but the app's own folder, run as the app's account.
Opening it still needs `file.read` and `file.write` on the node.

A node has at most one root per kind and folder. Approving a folder that is
already a root is refused and names the existing root. App roots need a
Linux node; Windows nodes have no per-app accounts.

### How an app root runs

```diagram
alt: Service file access through the root helper and file worker
flow:
  steps:
    - { label: agent, edge: "" }
    - { label: root helper, note: "finds the app's account itself, refuses chown, .forge and the folder, strips setuid/setgid, starts the worker, bounds and checks its answer", edge: "" }
    - { label: file worker, note: "runs as the app's uid and gid, checks its own ids, lowers limits, runs one operation", tone: accent, edge: "" }
    - { label: "app's folder" }
```

1. The control plane sends the app's environment and name, never an account.
2. The root helper looks the account up on the node. It accepts only an
   account that carries Forge's mark (`Orbyn Forge app <environment>/<app>`),
   has exactly the app's folder as its home and owns it, is not uid or gid
   `0`, has a `nologin` shell and has no privileged primary group.
3. The helper refuses what an app root never allows, then starts its own
   binary as
   [`forge-agent-helper file-worker`](/docs/reference/cli/forge-agent-helper/#forge-agent-helper-file-worker)
   under the app's uid and gid, with an empty environment and `/` as its
   working directory.
4. The worker checks in `/proc/self/status` that its real, effective, saved
   and filesystem ids are all the app's and that it holds no other group. It
   turns off core dumps, limits itself to 512 open files and to files of
   2 GiB and caps its CPU time.
5. Before it starts any thread, the worker confines itself with Landlock:
   from then on it can read and write only beneath the app's folder and its
   store, and read `/etc/passwd` and `/etc/group` for owner names. It cannot
   open network connections, reach abstract UNIX sockets or send signals to
   other processes, including the app's own. This holds even where the app's
   account could otherwise act, such as on world-readable files elsewhere on
   the node. Then it runs the operation with the same path rules as every
   other root.
6. The helper treats the worker as untrusted. It reads at most 48 MiB of
   answer in a strict format, stops the worker after 120 s (600 s for a
   search), runs at most 8 workers at once on a node, and never acts as root
   on anything the worker returns.

The audit event of each operation on an app root carries `app`
(`<environment>/<app>`, from the control plane) and `as_reported` (the account
the node says it used). See [Audit](#audit).

:::caution
The app's own processes share the worker's account. Where the kernel
setting `kernel.yama.ptrace_scope` is `0`, they can attach to a running
worker and change what it does, within the app's own rights. Set it to `1`;
see [Hardening](/docs/security/hardening/#file-explorer).
:::

Landlock needs Linux 5.13 or later with Landlock enabled. Forge uses it as
far as the kernel allows and logs a warning on the agent when it cannot use
it fully; the app account's own permissions apply either way:

| Kernel | Landlock |
| --- | --- |
| Ubuntu 22.04 and later | Enforced |
| Debian 12 (6.1) | Enforced, except for truncating files |
| RHEL 9 and its rebuilds | Enforced when the kernel enables Landlock; the agent's log says which |
| RHEL 8 and its rebuilds (4.18) | Not available: the app account's permissions only |

## Saving and conflicts

Opening a file returns a `version` token built from its identity, size and
timestamps. Saving sends that token. The node writes the new content to a
private temporary file in the same folder, verifies its size and SHA-256,
flushes it, gives it the old file's owner and mode (without setuid and
setgid), then renames it over the
original in one step. If the file changed since it was opened, the save is
refused and the console shows your edit beside the current file to merge.
Forge serializes its own saves to one path; a program on the node that
writes the file at the same moment is detected only through the version.

The editor keeps the file's encoding (UTF-8 with or without BOM, UTF-16,
Latin-1) and line endings. JSON is parsed before it is saved and marked
**Validated**. Applying configuration (reloading Nginx or a service) is not
part of saving.

## Uploads

Uploads go in chunks of 2 MiB, up to 3 at a time, which Forge writes to the
node in order in pieces of 256 KiB, to a private partial file
(`.forge-upload-<id>.part`, mode `0600`) next to the destination, which
listings hide. The browser computes the SHA-256 as it sends; the node checks
size and checksum before it moves the file into place. An interrupted upload
resumes from the byte the node reports. Only the user who started an upload
can continue it, permissions are checked on every chunk, and unfinished
uploads are removed after 24 hours.

An upload that may not replace a file is refused before any byte is sent
when the file already exists. If the target appears or changes while the
upload runs, the commit is refused and the node deletes the partial file at
once. A checksum mismatch keeps it, so the commit can be repeated with the
right checksum. When an upload starts in a folder, the node also deletes
partial files there that nobody has written to for 24 hours, such as the
leftovers of a server that was offline when their upload expired.

The browser uploads two files at a time from one queue; each of them sends
its chunks in parallel as above. Progress, speed and the remaining time come
from the chunks the server has acknowledged, so they reflect what reached
the node, not what left the browser.

## Trash and revisions

Deleting moves an entry to the trash unless you choose **Delete
permanently**. The trash and saved revisions live in Forge's agent data
directory (`/var/lib/forge-agent/files` or `C:\ProgramData\OrbynForge\files`), a
protected path that no root can reach. An app root keeps its own trash and
revisions in `/var/lib/forge-app-store/<environment>/<app>/`; see
[App roots](#app-roots).

| Kept | Limit |
| --- | --- |
| Trash | 30 days, 10 GiB per node, and another 10 GiB in each app root's store; the oldest go first |
| Revisions | The last 10 versions of each file saved from the console, up to 30 days, for files up to 2 MiB |

Restoring checks your permissions and the protected-path policy again and
puts the entry back where it was; the original folder must still exist.

## Search

Search runs on the node, inside the root, without following links. It skips
protected paths, binary files and files larger than the size limit, and
stops at the first limit it reaches; the result says whether it is complete.

| Limit | Value |
| --- | --- |
| Matches | 1,000 |
| Result size | 512 KiB |
| Time | 50 s |
| Bytes read | 2 GiB |
| File size | 1 MiB by default, at most 8 MiB |
| Depth | 32 by default, at most 64 |
| Searches | One per user per node; two per node |

Files are read in 64 KiB pieces, so a search never holds a whole file in
memory. On Linux searches run at low CPU and I/O priority. On Windows they
run at normal priority; the two-per-node limit bounds their cost. Search
patterns are not written to the audit log.

## Limits

| Limit | Value |
| --- | --- |
| Entries per listing page | 1–1000, default 500 |
| Entries read from one directory | 100,000; larger directories are listed partially |
| Bytes per read | 2 MiB through the API (256 KiB per read on the node) |
| Editor | Files up to 2 MiB; larger text files open read-only |
| Console download | 512 MiB |
| Upload | 50 GiB |
| Copy, or a move across filesystems | 10,000 entries or 1 GiB |
| Time per operation on the node | 10 minutes |

Reads travel inside agent heartbeats, which are capped at 1 MiB, so large
transfers are slow; for large files use [temporary SFTP access](#temporary-sftp-access).

## Permissions and owners

On Linux node and directory roots, administrators (`file.admin`, MFA) can
change permission bits and owners, optionally for everything inside a folder.
A preview shows what changes before anything is applied. Links, special
files and protected paths are skipped, never followed. The audit records the
mode or owner set and a sample of before and after values. Container roots
and Windows nodes show permissions read-only; on Windows the ACL is shown as
`icacls` prints it. On an [app root](#app-roots) owners cannot be changed,
and setuid and setgid bits are removed from any mode you set.

## Archives

You can pack a selection into a `.zip` or `.tar.gz`, download several items
as one archive, and extract `.zip`, `.tar`, `.tar.gz` and `.tgz` archives
into a new folder. Extraction is built to contain hostile archives:

- Every entry name goes through the same path rules as any other path:
  absolute paths, drive letters and `..` stop the extraction (zip-slip).
- Links, hard links and device files inside archives are skipped and
  reported.
- The entry count (20,000), total size (4 GiB) and compression ratio (200:1)
  are capped, which stops zip bombs.
- Entries are written into a hidden temporary folder next to the
  destination, through the same no-follow resolution, and the folder is
  renamed into place only when the whole archive checked out. A refused
  archive leaves nothing behind.
- By default the destination must be a new folder. With **replace**, an
  existing folder is replaced: the new tree is built next to it as above and
  then swapped in. On Linux the swap is one atomic `renameat2`
  (`RENAME_EXCHANGE`) call, so a site served from that folder always finds
  either the old or the new files, never none. On Windows, and on Linux
  filesystems without that call, Forge renames twice, which leaves a very
  short gap. The old content is deleted afterwards, so **replace** also needs
  `file.delete`.
- Files get mode `0644` and folders `0755`; setuid bits, modes and owners
  stored in an archive are ignored.

Archives are written to a private partial file and renamed into place, and
never overwrite an existing file.

An archive made for download goes to Forge's private store on the node and
is removed after it downloads, or after an hour. Only the user who made it
can download or delete it, through the same root of the same node. Its id
alone opens nothing.

## Temporary SFTP access

Forge can run its own SFTP server (the gateway) next to the control plane, so
you can give someone time-limited file access without a node account. You
issue **temporary access** for one or more roots of a node: read-only, or with
uploads, and optionally deletes. The user name is the access id
(`fg` and 16 hex digits); the password is a random token shown once. Forge
stores only its SHA-256 hash and compares it in constant time.

```diagram
alt: SFTP gateway path
flow:
  steps:
    - { label: SFTP client, edge: SSH }
    - { label: gateway (Forge), note: "password = token, SFTP only", tone: accent, edge: "" }
    - { label: file service, note: "as the grant's creator, narrowed by the grant", edge: "" }
    - { label: signed command, edge: "" }
    - { label: node }
```

- The gateway terminates SSH and offers only the `sftp` subsystem: no shell,
  command execution or port forwarding.
- Each SFTP request becomes a file-service operation, with the same root
  resolution, protected-path policy, node confinement and audit as the
  console. It is authorized as the access's owner (the person who issued
  it, or whoever last widened it), narrowed to
  the roots and operations the access allows. Effective access is the
  intersection of the grant, that person's current roles and each root's
  policy, so if they lose a role the access stops working too.
- The access is re-read on every operation. Revoking it, resetting its
  password or taking anything away from it closes its open sessions at once;
  every 5 seconds the gateway also closes sessions whose access expired or
  lost its creator's rights.
- Access normally expires within `[sftp] max_hours`. Access that never
  expires exists only when an administrator turns on
  `[sftp] allow_never_expiry`, and only users with `file.admin` can issue it.
- Issuing access needs an MFA check and every right it carries; the access
  can never do more than its issuer.
- Access to an [app root](#app-roots) runs on the node as the app's account,
  like every other operation on that root, so it can never do more than the
  app's account could.
- Failed logins are throttled per client address (10 in 5 minutes).
- Optional limits: client addresses (IPs or CIDR ranges), sessions at once,
  bandwidth per session and total transfer. A login takes a session slot as
  it succeeds, and each further SFTP channel on the same connection takes
  another, so a client cannot exceed the limit with parallel logins or extra
  channels.

The client sees one folder per root (`/node`, `/Vendor-Drop`). Uploads go to
private partial files and appear only when the file is closed, after a
SHA-256 check; writes must be sequential. Replacing an existing file needs
the delete right. `rm` moves files to the trash (containers have none).
Changing times or permissions through SFTP is accepted and ignored. FTP and
FTPS are not offered.

The gateway's host key (Ed25519) is created on first start in the data
directory (`keys/sftp_host_ed25519`). Its fingerprint is shown with each
access, so users can verify it. To rotate it, delete the file and restart
Forge; clients then warn about a changed key.

## Downloads in the browser

Downloads are always sent as `application/octet-stream` attachments with
`X-Content-Type-Options: nosniff` and `Content-Security-Policy: sandbox`.
Image previews are shown from `data:` URLs, which never run scripts, so an
HTML or SVG file from a node cannot run in the console's origin.

## Audit

Each listing, read, download and change is recorded with its path and size,
and each refusal with its reason. Events on an [app root](#app-roots) also
carry `app` and `as_reported`. File contents are never recorded. See
[Audit log](/docs/concepts/audit/).
