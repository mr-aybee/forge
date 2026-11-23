---
title: forge nodes files
description: Every forge nodes files command, which browses, reads, changes, archives and shares files on a node from the command line.
sidebar:
  order: 4
---

The `forge nodes files` commands run the [File Explorer](/docs/concepts/file-explorer/)
from a terminal. Each command calls the same [Files endpoints](/docs/reference/rest-api/#files)
as the console, so roots, Cedar permissions, MFA, protected paths and audit
apply the same way. Every command takes the [global flags](/docs/reference/cli/#global-flags).

These rules apply to every command on this page:

- `<node>` is a node id, host name or display name. A name that matches more
  than one node fails with exit `2`.
- Paths start with `/` and are relative to the root. `.` and `..` are
  refused. On a Windows node the first part of a path on the `node` root is
  the drive, as in `/C:/Users`.
- `--root` takes a root id from [`forge nodes files roots`](#forge-nodes-files-roots).
  Without it the command uses the `node` root (the whole filesystem), which
  also needs `file.node` (**Platform admin** and **Admin** roles). Operators
  work in approved roots; see [Approve file roots](/docs/guides/approve-file-roots/).
- `file.write`, `file.delete` and `file.admin` need an MFA-verified session.
  When the server answers `step_up_required`, the CLI asks for a one-time
  code on a terminal. Without a terminal, or with `--non-interactive`, the
  command exits with `5`; run `forge auth step-up` first.
- Read-only roots refuse every change with exit `4`.

## Commands

**Browse**

| Command | What it does |
| --- | --- |
| [`forge nodes files roots`](#forge-nodes-files-roots) | Lists the roots you can open on the node |
| [`forge nodes files roots add`](#forge-nodes-files-roots-add) | Approves a service or container folder as a root |
| [`forge nodes files roots remove`](#forge-nodes-files-roots-remove) | Removes an approved root |
| [`forge nodes files ls`](#forge-nodes-files-ls) | Lists a directory |
| [`forge nodes files stat`](#forge-nodes-files-stat) | Shows size, mode, owner and `version` of one entry |
| [`forge nodes files hash`](#forge-nodes-files-hash) | Prints the SHA-256 of a file |
| [`forge nodes files search`](#forge-nodes-files-search) | Searches file names and content under a path |

**Read and write**

| Command | What it does |
| --- | --- |
| [`forge nodes files cat`](#forge-nodes-files-cat) | Prints a file to stdout |
| [`forge nodes files download`](#forge-nodes-files-download) | Saves a file to your computer |
| [`forge nodes files upload`](#forge-nodes-files-upload) | Uploads a file in chunks, checked with SHA-256 on the node |
| [`forge nodes files save`](#forge-nodes-files-save) | Writes stdin or a local file to a file, with a version check |
| [`forge nodes files revisions`](#forge-nodes-files-revisions) | Lists the saved revisions of a file |
| [`forge nodes files touch`](#forge-nodes-files-touch) | Creates an empty file |
| [`forge nodes files mkdir`](#forge-nodes-files-mkdir) | Creates a directory |
| [`forge nodes files rename`](#forge-nodes-files-rename) | Renames an entry in its folder |
| [`forge nodes files mv`](#forge-nodes-files-mv) | Moves or renames an entry within a root |
| [`forge nodes files cp`](#forge-nodes-files-cp) | Copies an entry within a root |
| [`forge nodes files rm`](#forge-nodes-files-rm) | Moves an entry to the trash, or deletes it |
| [`forge nodes files bulk`](#forge-nodes-files-bulk) | Copies, moves, trashes or deletes many entries in one call |

**Permissions and archives**

| Command | What it does |
| --- | --- |
| [`forge nodes files chmod`](#forge-nodes-files-chmod) | Sets permission bits (Linux) |
| [`forge nodes files chown`](#forge-nodes-files-chown) | Sets owner and group (Linux) |
| [`forge nodes files acl`](#forge-nodes-files-acl) | Shows the Windows ACL or the Linux mode |
| [`forge nodes files compress`](#forge-nodes-files-compress) | Packs entries into a new `.zip` or `.tar.gz` on the node |
| [`forge nodes files extract`](#forge-nodes-files-extract) | Unpacks an archive into a new folder on the node |
| [`forge nodes files archive`](#forge-nodes-files-archive) | Downloads several entries as one archive |

**Trash**

| Command | What it does |
| --- | --- |
| [`forge nodes files trash list`](#forge-nodes-files-trash-list) | Lists the entries in the node's trash |
| [`forge nodes files trash restore`](#forge-nodes-files-trash-restore) | Puts one trash entry back where it was |
| [`forge nodes files trash purge`](#forge-nodes-files-trash-purge) | Deletes one trash entry, or empties the trash |

**SFTP access**

| Command | What it does |
| --- | --- |
| [`forge nodes files sftp info`](#forge-nodes-files-sftp-info) | Shows the SFTP gateway's host, port and host key fingerprint |
| [`forge nodes files sftp list`](#forge-nodes-files-sftp-list) | Lists temporary access grants on one node or across the fleet |
| [`forge nodes files sftp get`](#forge-nodes-files-sftp-get) | Shows one grant |
| [`forge nodes files sftp grant`](#forge-nodes-files-sftp-grant) | Issues temporary SFTP access and prints the password once |
| [`forge nodes files sftp update`](#forge-nodes-files-sftp-update) | Changes a grant's folders, permissions, restrictions, name or expiry; the password stays |
| [`forge nodes files sftp reset-password`](#forge-nodes-files-sftp-reset-password) | Replaces a grant's password and prints the new one once |
| [`forge nodes files sftp revoke`](#forge-nodes-files-sftp-revoke) | Revokes a grant and closes its open sessions |

### forge nodes files roots

Lists the roots you can open on the node: the whole node, approved
service directories and container directories, each with your
permissions on it. Needs `file.read` on the node; the `node` root appears
only with `file.node`.

**Synopsis**

```text
forge nodes files roots [OPTIONS] <NODE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |

**Examples**

```sh
# List the roots you can open on node-01
forge nodes files roots node-01

# Print only the root ids
forge -o json nodes files roots node-01 | jq -r '.result.roots[].id'
```

**Output**

```text
$ forge nodes files roots node-01
ID                           KIND         LABEL            PATH                           ACCESS
node                         node         Node filesystem  /                              read-write
fr_3f9a1c2b4d5e6f708192a3b4 service      xcweb            /srv/forge/apps/lab-test/xcweb read-write
```

```json
$ forge -o json nodes files roots node-01
{
  "result": {
    "mfa": true,
    "os": "linux",
    "roots": [
      {
        "base": "/",
        "id": "node",
        "kind": "node",
        "label": "Node filesystem",
        "permissions": { "admin": true, "delete": true, "read": true, "write": true },
        "read_only": false
      }
    ],
    "server_id": "01900000-0000-7000-8000-000000000004"
  },
  "schema": "forge.cli/v1"
}
```

`mfa` says whether your session is MFA-verified. Container roots also carry
`container`, and service roots `service_id`. To approve or remove
roots, see [`forge nodes files roots add`](#forge-nodes-files-roots-add) and
[`forge nodes files roots remove`](#forge-nodes-files-roots-remove).

Exit codes:

- `0`: success.
- `2`: the node name is ambiguous.
- `3`: not signed in.
- `4`: you may not read files on this node.
- `6`: the node does not exist.
- `8`: the node did not answer.

**See also**: [`forge nodes files ls`](#forge-nodes-files-ls), [Browse files on a node](/docs/guides/browse-files/), [Approve file roots](/docs/guides/approve-file-roots/).

### forge nodes files roots add

Approves a folder as a root, so people with file permissions on the node can
open it without the whole-node root: a service folder on the node, or a
path inside a container. Needs `file.admin` on the node. A new root is
read-only unless you pass `--writable`.

**Synopsis**

```text
forge nodes files roots add [OPTIONS] --label <LABEL> <NODE> <PATH>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<path>` | absolute path | — | yes | The folder on the node (`service`), or the path inside the container (`container`) | `/srv/forge/apps/lab-test/xcweb` |
| `--label <LABEL>` | string | — | yes | Name shown in the root list and the console | `--label xcweb` |
| `--kind <KIND>` | enum: `service`, `container` | `service` | no | What the folder belongs to | `--kind container` |
| `--container <CONTAINER>` | full container id | none | with `--kind container` | The container that holds `<path>` | `--container <container-id>` |
| `--writable` | flag | off (read-only) | no | Allow changes in this root, within each person's own file permissions | `--writable` |
| `--service <SERVICE>` | service id | none | no | The service this folder belongs to; `forge services files` finds roots by it | `--service <service-id>` |

**Examples**

```sh
# Approve an app folder as a writable root
forge nodes files roots add node-01 /srv/forge/apps/lab-test/xcweb --label xcweb --writable --service <service-id>
```

**Output**

The new root as a key/value list (`id`, `kind`, `label`, `spec`,
`read_only`, `service_id`, `created_at`). With `-o json`, the same object
in `result`.

Exit codes: `0` success; `2` a bad path or kind, or the node name is
ambiguous; `3` not signed in; `4` no `file.admin` on the node; `6` no such node.

:::caution
A writable root lets everyone with `file.write` on the node change that
folder. Approve only folders that belong to one service.
:::

### forge nodes files roots remove

Removes an approved root. The files stay; only access through the root ends.
The whole-node root cannot be removed. Asks first unless you pass `--yes`.
Needs `file.admin` on the node.

**Synopsis**

```text
forge nodes files roots remove [OPTIONS] <NODE> <ROOT>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<root>` | root id (`fr_…`) | — | yes | The root, from `forge nodes files roots` | `fr_3f9a1c2b4d5e6f708192a3b4` |
| `--yes` | flag | off | no | Skip the confirmation prompt (needed without a terminal) | `--yes` |

**Examples**

```sh
forge nodes files roots remove node-01 fr_3f9a1c2b4d5e6f708192a3b4 --yes
```

**Output**

Nothing on success. With `-o json`, `result` is `null`.

Exit codes: `0` success; `1` you did not answer `yes`; `2` no `--yes` without
a terminal; `3` not signed in; `4` no `file.admin`; `6` no such node or root.

### forge nodes files ls

Lists one directory, sorted by name. Needs `file.read` on the root.

**Synopsis**

```text
forge nodes files ls [OPTIONS] <NODE> [PATH]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<path>` | path | `""` (the top of the root) | no | The directory to list | `/var/log` |
| `--root` | string (root id) | `node` | no | The root the path is in | `--root <root-id>` |
| `-a`, `--all` | flag | off | no | Include hidden entries (dotfiles, hidden files on Windows) | `-a` |
| `--limit` | integer, 1–1000 | `1000` | no | Most entries to show | `--limit 50` |
| `--hidden` | flag | off | no | Include hidden entries (same as `--all`) | `--hidden` |
| `--offset <N>` | integer | `0` | no | Skip this many entries first. When there are more, stderr names the next offset | `--offset 1000` |

**Examples**

```sh
# List the top of the node root
forge nodes files ls node-01

# List /var/log, including hidden entries
forge nodes files ls node-01 /var/log -a

# List the top of an approved root, at most 50 entries
forge nodes files ls node-01 / --root <root-id> --limit 50
```

**Output**

The table shows `KIND`, `MODE`, `OWNER`, `GROUP`, `SIZE`, `MODIFIED` and
`NAME`. `MODE`, `OWNER` and `GROUP` are Linux only. When the directory has
more entries than `--limit`, the CLI says so on stderr.

```text
$ forge nodes files ls node-01 /var/log
KIND     MODE       OWNER  GROUP            SIZE    MODIFIED          NAME
symlink  rwxrwxrwx  root   root             0       2026-11-11 23:51  README
file     rw-r--r--  root   root             18027   2026-11-13 07:35  alternatives.log
dir      rwxr-xr-x  root   root             0       2026-11-13 07:36  apt
file     rw-rw----  root   utmp             0       2026-11-11 23:51  btmp
file     rw-r--r--  root   root             211311  2026-11-13 07:36  dpkg.log
…
file     rw-rw-r--  root   utmp             3840    2026-11-13 14:32  wtmp
file     rw-r--r--  root   root             8192    2026-11-13 14:32  wtmp.db
```

With `-o json` you get the full page: `{path, offset, total, truncated,
skipped_names, entries}`. `mode` is a decimal number (`420` is `0644`).

```json
$ forge -o json nodes files ls node-01 /var/log
{
  "result": {
    "entries": [
      {
        "group": "root",
        "hidden": false,
        "kind": "symlink",
        "link_target": "../../usr/share/doc/systemd/README.logs",
        "mode": 511,
        "modified_ms": 1794441111085,
        "mount": false,
        "name": "README",
        "owner": "root",
        "protected": false,
        "readonly": false,
        "size": 0
      },
      {
        "group": "root",
        "hidden": false,
        "kind": "file",
        "mode": 420,
        "modified_ms": 1794555323109,
        "mount": false,
        "name": "alternatives.log",
        "owner": "root",
        "protected": false,
        "readonly": false,
        "size": 18027
      },
      …
    ],
    …
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: success.
- `2`: invalid path, limit or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, or a protected path.
- `6`: the node, root or directory does not exist.
- `8`: the node did not answer.

**See also**: [`forge nodes files stat`](#forge-nodes-files-stat), [`forge nodes files search`](#forge-nodes-files-search), [Browse files on a node](/docs/guides/browse-files/).

### forge nodes files stat

Shows one entry's metadata and its `version` token. Links are reported, not
followed. Needs `file.read` on the root.

Pass the `version` to [`save --if-version`](#forge-nodes-files-save) or
[`upload --if-version`](#forge-nodes-files-upload) so a change made since you
read the file is not overwritten.

**Synopsis**

```text
forge nodes files stat [OPTIONS] <NODE> <PATH>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<path>` | path | — | yes | The file or directory | `/etc/hostname` |
| `--root` | string (root id) | `node` | no | The root the path is in | `--root <root-id>` |

**Examples**

```sh
# Show the details of /etc/hostname
forge nodes files stat node-01 /etc/hostname

# Keep only the version token for a later save
forge -o json nodes files stat node-01 /etc/hostname | jq -r '.result.version'
```

**Output**

```text
$ forge nodes files stat node-01 /etc/hostname
{
  "entry": {
    "group": "root",
    "hidden": false,
    "kind": "file",
    "mode": 420,
    "modified_ms": 1794441119793,
    "mount": false,
    "name": "hostname",
    "owner": "root",
    "protected": false,
    "readonly": false,
    "size": 6
  },
  "path": "/etc/hostname",
  "version": "802-40d27-6-6af4ff9f.2f516076-6af4ff9f.2f516076"
}
```

```json
$ forge -o json nodes files stat node-01 /etc/hostname
{
  "result": {
    "entry": {
      "group": "root",
      "hidden": false,
      "kind": "file",
      "mode": 420,
      "modified_ms": 1794441119793,
      "mount": false,
      "name": "hostname",
      "owner": "root",
      "protected": false,
      "readonly": false,
      "size": 6
    },
    "path": "/etc/hostname",
    "version": "802-40d27-6-6af4ff9f.2f516076-6af4ff9f.2f516076"
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: success.
- `2`: invalid path or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, or a protected path.
- `6`: the node, root or path does not exist.
- `8`: the node did not answer.

**See also**: [`forge nodes files save`](#forge-nodes-files-save), [`forge nodes files hash`](#forge-nodes-files-hash).

### forge nodes files hash

Prints the SHA-256 of a file, computed on the node. Needs `file.read` on the
root.

**Synopsis**

```text
forge nodes files hash [OPTIONS] <NODE> <PATH>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<path>` | path | — | yes | The file | `/etc/hostname` |
| `--root` | string (root id) | `node` | no | The root the path is in | `--root <root-id>` |

**Examples**

```sh
# Hash /etc/hostname
forge nodes files hash node-01 /etc/hostname

# Compare with a local copy
forge -o json nodes files hash node-01 /etc/hostname | jq -r '.result.sha256'
sha256sum ./hostname
```

**Output**

```text
$ forge nodes files hash node-01 /etc/hostname
{
  "path": "/etc/hostname",
  "sha256": "d959ea7e0e1264565b8a3bd4505bfcc9cb49eb37b2e8cdd9fff942c4b9c0cad0",
  "size": 6,
  "version": "802-40d27-6-6af4ff9f.2f516076-6af4ff9f.2f516076"
}
```

```json
$ forge -o json nodes files hash node-01 /etc/hostname
{
  "result": {
    "path": "/etc/hostname",
    "sha256": "d959ea7e0e1264565b8a3bd4505bfcc9cb49eb37b2e8cdd9fff942c4b9c0cad0",
    "size": 6,
    "version": "802-40d27-6-6af4ff9f.2f516076-6af4ff9f.2f516076"
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: success.
- `2`: invalid path, a directory or link, or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, or a protected path.
- `6`: the node, root or file does not exist.
- `8`: the node did not answer.

**See also**: [`forge nodes files stat`](#forge-nodes-files-stat), [`forge nodes files download`](#forge-nodes-files-download).

### forge nodes files search

Searches file names, content or both under a path, on the node. Needs
`file.read` on the root. Give `--name`, `--content` or both; without either
the command fails with exit `2`.

The search skips protected paths, binary files and files over 1 MiB, does
not follow links, and stops at 1,000 matches or 50 seconds. You can run one
search at a time on each node. See [Search](/docs/concepts/file-explorer/#search) for
all limits.

**Synopsis**

```text
forge nodes files search [OPTIONS] <NODE> [PATH]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<path>` | path | `""` (the top of the root) | no | Where to search | `/etc` |
| `--root` | string (root id) | `node` | no | The root the path is in | `--root <root-id>` |
| `--name` | string (glob) | — | one of `--name`, `--content` | Pattern on file names | `--name '*.conf'` |
| `--content` | string | — | one of `--name`, `--content` | Text to find in files; a regular expression with `--regex` | `--content listen` |
| `--regex` | flag | off | no | Treat `--content` as a regular expression | `--regex` |
| `--case-sensitive` | flag | off | no | Match case exactly | `--case-sensitive` |

**Examples**

```sh
# Find files whose name starts with "host" under /etc
forge nodes files search node-01 /etc --name 'host*'

# Find .conf files that mention "listen"
forge nodes files search node-01 /etc --name '*.conf' --content listen

# Find a regular expression, matching case
forge nodes files search node-01 /srv/app --content 'API_(KEY|URL)' --regex --case-sensitive
```

**Output**

`files[].path` is relative to the searched path. `status` is `complete`,
`truncated` or `timeout`. Content searches add `matches: [{line, text}]` to
each file.

```text
$ forge nodes files search node-01 /etc --name 'host*'
{
  "files": [
    {
      "modified_ms": 1744297585000,
      "path": "apparmor.d/abstractions/hosts_access",
      "size": 511
    },
    {
      "modified_ms": 1784307900000,
      "path": "host.conf",
      "size": 9
    },
    …
  ],
  "matches": 6,
  "next": null,
  "path": "/etc",
  "scanned_bytes": 0,
  "scanned_files": 6,
  "status": "complete"
}
```

```json
$ forge -o json nodes files search node-01 /etc --name 'host*'
{
  "result": {
    "files": [
      { "modified_ms": 1744297585000, "path": "apparmor.d/abstractions/hosts_access", "size": 511 },
      { "modified_ms": 1784307900000, "path": "host.conf", "size": 9 },
      { "modified_ms": 1794441119793, "path": "hostname", "size": 6 },
      { "modified_ms": 1794561043593, "path": "hosts", "size": 423 },
      { "modified_ms": 1794441520537, "path": "hosts.allow", "size": 411 },
      { "modified_ms": 1794441520537, "path": "hosts.deny", "size": 711 }
    ],
    "matches": 6,
    "next": null,
    "path": "/etc",
    "scanned_bytes": 0,
    "scanned_files": 6,
    "status": "complete"
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: success, also when the search stopped early (check `status`).
- `1`: a search of yours already runs on this node, or the node runs two searches.
- `2`: neither `--name` nor `--content`, an invalid pattern or path, or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied.
- `6`: the node, root or path does not exist.
- `8`: the node did not answer.

**See also**: [Search in the File Explorer](/docs/guides/manage-files/#search), [`forge nodes files ls`](#forge-nodes-files-ls).

### forge nodes files cat

Prints a file's bytes to stdout, text or binary, until the end or
`--max-bytes`. Needs `file.read` on the root.

The CLI reads the file in pieces of 2 MiB and pins every piece to the
version of the first one. If the file changes during the read, the command
fails instead of mixing old and new content. When it stops at `--max-bytes`,
it says so on stderr.

**Synopsis**

```text
forge nodes files cat [OPTIONS] <NODE> <PATH>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<path>` | path | — | yes | The file | `/etc/hostname` |
| `--root` | string (root id) | `node` | no | The root the path is in | `--root <root-id>` |
| `--max-bytes` | integer (bytes) | `10485760` (10 MiB) | no | Stop after this many bytes | `--max-bytes 4096` |

**Examples**

```sh
# Print /etc/hostname
forge nodes files cat node-01 /etc/hostname

# Print the first 4 KiB of a log
forge nodes files cat node-01 /var/log/dpkg.log --max-bytes 4096

# Show the last lines of a log
forge nodes files cat node-01 /var/log/dpkg.log | tail -n 20
```

**Output**

The file's content, unchanged. `-o json` does not wrap it: the output is the
same raw content.

```text
$ forge nodes files cat node-01 /etc/hostname
node-01
```

Exit codes:

- `0`: success.
- `2`: invalid path, a directory or link, or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, or a protected path.
- `6`: the node, root or file does not exist.
- `7`: the file changed during the read.
- `8`: the node did not answer.

**See also**: [`forge nodes files download`](#forge-nodes-files-download), [`forge nodes files save`](#forge-nodes-files-save).

### forge nodes files download

Saves a file from the node to a local path. Needs `file.read` on the root.

The CLI writes to a `.forge-part` file next to `<local>` and renames it when
the transfer is complete, so a failed download leaves no half file. It never
replaces an existing local file unless you pass `--force`. Downloads through
Forge are limited to 512 MiB and fail if the file changes during the
transfer; use [SFTP access](#forge-nodes-files-sftp-grant) for larger files.

**Synopsis**

```text
forge nodes files download [OPTIONS] <NODE> <REMOTE> <LOCAL>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<remote>` | path | — | yes | The file on the node | `/etc/nginx/nginx.conf` |
| `<local>` | local path | — | yes | Where to save it | `./nginx.conf` |
| `--root` | string (root id) | `node` | no | The root `<remote>` is in | `--root <root-id>` |
| `--force` | flag | off | no | Replace `<local>` if it exists | `--force` |

**Examples**

```sh
# Download the Nginx configuration
forge nodes files download node-01 /etc/nginx/nginx.conf ./nginx.conf

# Download again, replacing the local copy
forge nodes files download node-01 /etc/nginx/nginx.conf ./nginx.conf --force
```

**Output**

Shape from the server's schema; not captured on a live system.

```json
{
  "bytes": 1447,
  "file": "./nginx.conf"
}
```

With `-o json` the same object is under `result`, next to
`"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `1`: the file is over 512 MiB.
- `2`: invalid path, a directory, or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, or a protected path.
- `6`: the node, root or file does not exist.
- `7`: `<local>` exists and `--force` is missing.
- `8`: the node did not answer.
- `9`: the transfer broke off, or the file changed during it.

**See also**: [`forge nodes files archive`](#forge-nodes-files-archive), [`forge nodes files cat`](#forge-nodes-files-cat), [Browse files on a node](/docs/guides/browse-files/).

### forge nodes files upload

Uploads a local file to the node in chunks. Needs `file.read` and
`file.write` on the root, with an MFA-verified session.

The node writes the chunks to a private partial file next to the
destination, checks the size and the SHA-256 the CLI computed, then moves
the file into place. Without `--overwrite` or `--if-version` the upload
refuses an existing file before any bytes are sent (exit `7`; the error suggests `--overwrite`). A missing destination folder is named in full in the error. A replaced file keeps its owner and mode, without
setuid and setgid. Uploads are limited to 50 GiB. If a chunk fails, the CLI
cancels the upload and the node removes the partial file.

**Synopsis**

```text
forge nodes files upload [OPTIONS] <NODE> <LOCAL> <REMOTE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<local>` | local path | — | yes | The file to upload | `./site.tar.gz` |
| `<remote>` | path | — | yes | Where to write it on the node | `/srv/app/site.tar.gz` |
| `--root` | string (root id) | `node` | no | The root `<remote>` is in | `--root <root-id>` |
| `--overwrite` | flag | off | no | Replace an existing file. Cannot be combined with `--if-version` | `--overwrite` |
| `--if-version` | string (version token) | — | no | Replace the file only if it still has this version, from [`stat`](#forge-nodes-files-stat) | `--if-version <version>` |

**Examples**

```sh
# Upload a new file
forge nodes files upload node-01 ./site.tar.gz /srv/app/site.tar.gz

# Upload into an approved root, replacing an existing file
forge nodes files upload node-01 ./app.env /app.env --root <root-id> --overwrite

# Replace a file only if nobody changed it since you read it
v=$(forge -o json nodes files stat node-01 /srv/app/app.env | jq -r '.result.version')
forge nodes files upload node-01 ./app.env /srv/app/app.env --if-version "$v"
```

**Output**

In table mode the CLI shows progress on stderr. Then it prints the result.

Shape from the server's schema; not captured on a live system.

```json
{
  "path": "/srv/app/site.tar.gz",
  "replaced": false,
  "size": 52428800,
  "version": "<version>"
}
```

With `-o json` the same object is under `result`, next to
`"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `1`: the file is too large or the node's disk is full.
- `2`: `<local>` cannot be read, an invalid path, a checksum mismatch, or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, a read-only root, or a protected path.
- `5`: MFA needed.
- `6`: the node or root does not exist.
- `7`: the file exists, or it changed since `--if-version`.
- `8`: the node did not answer.

**See also**: [`forge nodes files save`](#forge-nodes-files-save), [Upload files and folders](/docs/guides/manage-files/#upload-files-and-folders), [Uploads](/docs/concepts/file-explorer/#uploads).

### forge nodes files save

Writes stdin, or the local file in `--from`, to a file on the node in one
atomic step. Needs `file.read` and `file.write` on the root, with an
MFA-verified session.

Without `--if-version` the command creates a new file and fails if the file
exists. With `--if-version` it replaces the file only if it still has that
version. Saves are limited to 2 MiB; use [`upload`](#forge-nodes-files-upload)
for larger files. The node keeps the replaced content as a
[revision](#forge-nodes-files-revisions).

**Synopsis**

```text
forge nodes files save [OPTIONS] <NODE> <PATH>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<path>` | path | — | yes | The file to write | `/srv/app/app.env` |
| `--root` | string (root id) | `node` | no | The root the path is in | `--root <root-id>` |
| `--from` | local path | stdin | no | Local file to save | `--from ./app.env` |
| `--if-version` | string (version token) | — | no | The version you read, from [`stat`](#forge-nodes-files-stat). Omit it to create a new file | `--if-version <version>` |

**Examples**

```sh
# Create a new file from stdin
echo "hello" | forge nodes files save node-01 /srv/app/hello.txt

# Edit a file safely: read it, change it, save it with its version
v=$(forge -o json nodes files stat node-01 /srv/app/app.env | jq -r '.result.version')
forge nodes files cat node-01 /srv/app/app.env > app.env
# … edit app.env …
forge nodes files save node-01 /srv/app/app.env --from ./app.env --if-version "$v"
```

**Output**

Shape from the server's schema; not captured on a live system.

```json
{
  "path": "/srv/app/app.env",
  "replaced": true,
  "size": 214,
  "version": "<version>"
}
```

With `-o json` the same object is under `result`, next to
`"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `1`: the content is over 2 MiB.
- `2`: `--from` cannot be read, an invalid path, or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, a read-only root, or a protected path.
- `5`: MFA needed.
- `6`: the node or root does not exist.
- `7`: the file exists (no `--if-version`), or it changed since that version.
- `8`: the node did not answer.

**See also**: [`forge nodes files stat`](#forge-nodes-files-stat), [Edit a file](/docs/guides/manage-files/#edit-a-file), [Saving and conflicts](/docs/concepts/file-explorer/#saving-and-conflicts).

### forge nodes files revisions

Lists the saved revisions of a file, newest first. Needs `file.read` on the
root. Forge keeps the last 10 versions of each file saved through Forge, up
to 30 days, for files up to 2 MiB. Container roots keep no revisions.

**Synopsis**

```text
forge nodes files revisions [OPTIONS] <NODE> <PATH>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<path>` | path | — | yes | The file | `/srv/app/app.env` |
| `--root` | string (root id) | `node` | no | The root the path is in | `--root <root-id>` |

**Examples**

```sh
# List the revisions of a file
forge nodes files revisions node-01 /srv/app/app.env

# Count them
forge -o json nodes files revisions node-01 /srv/app/app.env | jq '.result.revisions | length'
```

**Output**

Shape from the server's schema; not captured on a live system.

```json
{
  "path": "/srv/app/app.env",
  "revisions": [
    { "id": "<revision-id>", "saved_at": 1794555323109, "size": 214 }
  ]
}
```

`saved_at` is in epoch milliseconds. With `-o json` the same object is under
`result`, next to `"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `2`: invalid path or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied.
- `6`: the node or root does not exist.
- `8`: the node did not answer.

**See also**: [Go back to an earlier version](/docs/guides/manage-files/#go-back-to-an-earlier-version), [Trash and revisions](/docs/concepts/file-explorer/#trash-and-revisions).

### forge nodes files touch

Creates an empty file. Fails if the file exists. Needs `file.read` and
`file.write` on the root, with an MFA-verified session.

**Synopsis**

```text
forge nodes files touch [OPTIONS] <NODE> <PATH>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<path>` | path | — | yes | The file to create | `/srv/app/maintenance.flag` |
| `--root` | string (root id) | `node` | no | The root the path is in | `--root <root-id>` |

**Examples**

```sh
# Create an empty file
forge nodes files touch node-01 /srv/app/maintenance.flag

# Create it in an approved root
forge nodes files touch node-01 /maintenance.flag --root <root-id>
```

**Output**

Shape from the server's schema; not captured on a live system.

```json
{
  "path": "/srv/app/maintenance.flag",
  "replaced": false,
  "size": 0,
  "version": "<version>"
}
```

With `-o json` the same object is under `result`, next to
`"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `2`: invalid path or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, a read-only root, or a protected path.
- `5`: MFA needed.
- `6`: the node, root or parent folder does not exist.
- `7`: the file exists.
- `8`: the node did not answer.

**See also**: [`forge nodes files mkdir`](#forge-nodes-files-mkdir), [`forge nodes files save`](#forge-nodes-files-save).

### forge nodes files mkdir

Creates one directory. The parent must exist unless you pass `--parents`.
When it is missing, the error names the full parent path, for example
`parent folder /srv/forge/apps/lab-test of /srv/forge/apps/lab-test/site does
not exist on node-01; use --parents to create it`. Needs `file.read` and
`file.write` on the root, with an MFA-verified session.

**Synopsis**

```text
forge nodes files mkdir [OPTIONS] <NODE> <PATH>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<path>` | path | — | yes | The directory to create | `/srv/app/uploads` |
| `--root` | string (root id) | `node` | no | The root the path is in | `--root <root-id>` |
| `--parents`, `-p` | flag | off | no | Create missing parent folders too; an existing folder is then not an error | `-p` |

**Examples**

```sh
# Create a directory
forge nodes files mkdir node-01 /srv/app/uploads

# Create it in an approved root
forge nodes files mkdir node-01 /uploads --root <root-id>

# Create it with every missing parent
forge nodes files mkdir node-01 /srv/forge/apps/lab-test/xcweb/site -p
```

**Output**

Shape from the server's schema; not captured on a live system.

```json
{
  "path": "/srv/app/uploads"
}
```

With `-o json` the same object is under `result`, next to
`"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `2`: invalid path or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, a read-only root, or a protected path.
- `5`: MFA needed.
- `6`: the node, root or parent does not exist.
- `7`: the directory exists.
- `8`: the node did not answer.

**See also**: [`forge nodes files touch`](#forge-nodes-files-touch), [Create a folder or a file](/docs/guides/manage-files/#create-a-folder-or-a-file).

### forge nodes files rename

Renames an entry in place: the new name stays in the same folder. Needs
`file.read`, `file.write` and `file.delete` on the root, with an
MFA-verified session.

`<new_name>` is a name, not a path. A name with `/` or `\`, or `.` or `..`,
fails with exit `2`. Use [`mv`](#forge-nodes-files-mv) to move to another
folder.

**Synopsis**

```text
forge nodes files rename [OPTIONS] <NODE> <PATH> <NEW_NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<path>` | path | — | yes | The entry to rename | `/srv/app/app.conf` |
| `<new_name>` | string (file name) | — | yes | The new name | `app.conf.old` |
| `--root` | string (root id) | `node` | no | The root the path is in | `--root <root-id>` |
| `--overwrite` | flag | off | no | Replace an existing entry with the new name | `--overwrite` |

**Examples**

```sh
# Rename a file in its folder
forge nodes files rename node-01 /srv/app/app.conf app.conf.old

# Rename it, replacing an existing entry with that name
forge nodes files rename node-01 /srv/app/app.conf app.conf.old --overwrite
```

**Output**

Shape from the server's schema; not captured on a live system.

```json
{
  "from": "/srv/app/app.conf",
  "kind": "file",
  "to": "/srv/app/app.conf.old"
}
```

With `-o json` the same object is under `result`, next to
`"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `2`: `<new_name>` is not a plain name, an invalid path, or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, a read-only root, or a protected path.
- `5`: MFA needed.
- `6`: the node, root or entry does not exist.
- `7`: an entry with the new name exists and `--overwrite` is missing.
- `8`: the node did not answer.

**See also**: [`forge nodes files mv`](#forge-nodes-files-mv), [Rename, copy and move](/docs/guides/manage-files/#rename-copy-and-move).

### forge nodes files mv

Moves or renames a file or directory within one root. Needs `file.read`,
`file.write` and `file.delete` on the root, with an MFA-verified session.
Across filesystems the node copies, then removes the source, up to 10,000
entries or 1 GiB.

**Synopsis**

```text
forge nodes files mv [OPTIONS] <NODE> <FROM> <TO>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<from>` | path | — | yes | The entry to move | `/srv/app/build` |
| `<to>` | path | — | yes | The new full path, not the target folder | `/srv/archive/build` |
| `--root` | string (root id) | `node` | no | The root both paths are in | `--root <root-id>` |
| `--overwrite` | flag | off | no | Replace an existing entry at `<to>` | `--overwrite` |

**Examples**

```sh
# Move a directory to another folder
forge nodes files mv node-01 /srv/app/build /srv/archive/build

# Move a file and replace the one at the destination
forge nodes files mv node-01 /srv/app/new.conf /srv/app/app.conf --overwrite
```

**Output**

Shape from the server's schema; not captured on a live system.

```json
{
  "from": "/srv/app/build",
  "kind": "dir",
  "to": "/srv/archive/build"
}
```

With `-o json` the same object is under `result`, next to
`"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `1`: a move across filesystems is over 1 GiB.
- `2`: invalid path or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, a read-only root, or a protected path.
- `5`: MFA needed.
- `6`: the node, root or source does not exist.
- `7`: the destination exists and `--overwrite` is missing, or another change to the path is running.
- `8`: the node did not answer.

**See also**: [`forge nodes files rename`](#forge-nodes-files-rename), [`forge nodes files bulk`](#forge-nodes-files-bulk).

### forge nodes files cp

Copies a file or directory tree within one root, up to 10,000 entries or
1 GiB. Needs `file.read` and `file.write` on the root, with an MFA-verified
session.

**Synopsis**

```text
forge nodes files cp [OPTIONS] <NODE> <FROM> <TO>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<from>` | path | — | yes | The entry to copy | `/srv/app/app.conf` |
| `<to>` | path | — | yes | The full path of the copy | `/srv/app/app.conf.bak` |
| `--root` | string (root id) | `node` | no | The root both paths are in | `--root <root-id>` |
| `--overwrite` | flag | off | no | Replace an existing entry at `<to>` | `--overwrite` |

**Examples**

```sh
# Back up a configuration file
forge nodes files cp node-01 /srv/app/app.conf /srv/app/app.conf.bak

# Copy a directory tree, replacing an older copy
forge nodes files cp node-01 /srv/app/public /srv/app/public.bak --overwrite
```

**Output**

Shape from the server's schema; not captured on a live system.

```json
{
  "bytes": 2048,
  "entries": 1,
  "from": "/srv/app/app.conf",
  "to": "/srv/app/app.conf.bak"
}
```

With `-o json` the same object is under `result`, next to
`"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `1`: the copy is over 10,000 entries or 1 GiB.
- `2`: invalid path or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, a read-only root, or a protected path.
- `5`: MFA needed.
- `6`: the node, root or source does not exist.
- `7`: the destination exists and `--overwrite` is missing.
- `8`: the node did not answer.

**See also**: [`forge nodes files mv`](#forge-nodes-files-mv), [`forge nodes files bulk`](#forge-nodes-files-bulk).

### forge nodes files rm

Moves an entry to the node's trash, or deletes it for good with
`--permanent`. Needs `file.read` and `file.delete` on the root, with an
MFA-verified session. The command asks before it deletes unless you pass
`--yes`.

Container roots have no trash: use `--permanent` there. Entries too large to
move to the trash from another filesystem must also be deleted permanently.

:::caution
`--permanent` removes the entry and everything inside it. It cannot be
restored.
:::

**Synopsis**

```text
forge nodes files rm [OPTIONS] <NODE> <PATH>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<path>` | path | — | yes | The entry to delete | `/srv/app/old` |
| `--root` | string (root id) | `node` | no | The root the path is in | `--root <root-id>` |
| `--permanent` | flag | off | no | Delete permanently instead of moving to the trash | `--permanent` |
| `--yes` | flag | off | no | Do not ask first. Required with `--non-interactive` or without a terminal | `--yes` |

**Examples**

```sh
# Move a directory to the trash (asks first)
forge nodes files rm node-01 /srv/app/old

# Move it to the trash without asking, in a script
forge nodes files rm node-01 /srv/app/old --yes

# Delete a file in a container root for good
forge nodes files rm node-01 /tmp/cache.bin --root <root-id> --permanent --yes
```

**Output**

Shape from the server's schema; not captured on a live system.

Moved to the trash (keep `trash_id` for [`trash restore`](#forge-nodes-files-trash-restore)):

```json
{
  "path": "/srv/app/old",
  "size": 40960,
  "trash_id": "<trash-id>"
}
```

Deleted with `--permanent` (`entries` is the number of removed entries):

```json
{
  "entries": 12,
  "path": "/srv/app/old",
  "permanent": true
}
```

With `-o json` the same object is under `result`, next to
`"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `1`: you did not answer `yes` at the prompt.
- `2`: no `--yes` where no prompt is possible, an invalid path, a container root without `--permanent`, or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, a read-only root, or a protected path.
- `5`: MFA needed.
- `6`: the node, root or entry does not exist.
- `8`: the node did not answer.

**See also**: [`forge nodes files trash list`](#forge-nodes-files-trash-list), [`forge nodes files bulk`](#forge-nodes-files-bulk), [Delete and restore](/docs/guides/manage-files/#delete-and-restore).

### forge nodes files bulk

Runs one action on 1–500 paths in one call and prints one result per path.
Needs the permissions of the action on the root, with an MFA-verified
session:

| `<action>` | Does | Permissions |
| --- | --- | --- |
| `copy` | Copies each path into the `--to` folder | `file.read`, `file.write` |
| `move` | Moves each path into the `--to` folder | `file.read`, `file.write`, `file.delete` |
| `trash` | Moves each path to the trash | `file.read`, `file.delete` |
| `delete` | Deletes each path permanently | `file.read`, `file.delete` |

`copy` and `move` need `--to`; `trash` and `delete` take no `--to`. `trash`
and `delete` ask first unless you pass `--yes`. A failed path does not stop
the others, but a missing MFA step-up stops the whole batch.

**Synopsis**

```text
forge nodes files bulk [OPTIONS] <NODE> <ACTION> <PATHS>...
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<action>` | enum: `copy`, `move`, `trash`, `delete` | — | yes | What to do with each path | `move` |
| `<paths>` | paths, 1–500, space-separated | — | yes | The entries | `/srv/a.log /srv/b.log` |
| `--to` | path | — | for `copy`, `move` | The destination folder; each entry keeps its name | `--to /srv/archive` |
| `--root` | string (root id) | `node` | no | The root all paths are in | `--root <root-id>` |
| `--overwrite` | flag | off | no | Replace existing entries in `--to` | `--overwrite` |
| `--yes` | flag | off | no | Do not ask first (`trash`, `delete`) | `--yes` |

**Examples**

```sh
# Move two logs into an archive folder
forge nodes files bulk node-01 move /srv/app/a.log /srv/app/b.log --to /srv/archive

# Copy files, replacing older copies
forge nodes files bulk node-01 copy /srv/app/a.conf /srv/app/b.conf --to /srv/backup --overwrite

# Move several entries to the trash without asking
forge nodes files bulk node-01 trash /srv/app/tmp1 /srv/app/tmp2 --yes
```

**Output**

Shape from the server's schema; not captured on a live system.

The table shows `OK`, `PATH` and `ERROR` for each path:

```text
OK     PATH             ERROR
true   /srv/app/a.log
false  /srv/app/b.log   the destination already exists
```

With `-o json`:

```json
{
  "result": {
    "failed": 1,
    "ok": 1,
    "results": [
      { "ok": true, "path": "/srv/app/a.log" },
      { "error": "the destination already exists", "ok": false, "path": "/srv/app/b.log" }
    ]
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: every path succeeded.
- `1`: at least one path failed (the results show which), or you did not answer `yes` at the prompt.
- `2`: `--to` missing for `copy` or `move`, or given for `trash` or `delete`; an unknown action; no `--yes` where no prompt is possible; more than 500 paths; or ambiguous node name.
- `3`: not signed in.
- `5`: MFA needed; the batch stopped.
- `6`: the node or root does not exist.
- `8`: the node did not answer.

**See also**: [`forge nodes files rm`](#forge-nodes-files-rm), [`forge nodes files archive`](#forge-nodes-files-archive), [Select several items](/docs/guides/manage-files/#select-several-items).

### forge nodes files chmod

Sets permission bits on a file or directory, and with `-R` on everything
inside it. Needs `file.read` and `file.admin` on the root, with an
MFA-verified session (**Platform admin** and **Admin** roles).

Works on Linux nodes, on the `node` root and approved service roots.
Container roots and Windows nodes refuse it. Links, special files and
protected paths are skipped, never followed. One call changes at most
200,000 entries. Use `--dry-run` first to see what would change.

`<mode>` is octal, up to `7777`: `644`, `0644` and `0o4750` are all valid.

:::caution
A recursive change applies at once to everything below the path. Run it
with `--dry-run` first and check the counts.
:::

**Synopsis**

```text
forge nodes files chmod [OPTIONS] <NODE> <MODE> <PATH>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<mode>` | octal mode | — | yes | The permission bits to set | `0750` |
| `<path>` | path | — | yes | The file or directory | `/srv/app` |
| `--root` | string (root id) | `node` | no | The root the path is in | `--root <root-id>` |
| `-R`, `--recursive` | flag | off | no | Also everything inside folders | `-R` |
| `--dry-run` | flag | off | no | Only count what would change | `--dry-run` |

**Examples**

```sh
# Make one file readable by everyone
forge nodes files chmod node-01 0644 /srv/app/index.html

# Preview a recursive change, then apply it
forge nodes files chmod node-01 0750 /srv/app -R --dry-run
forge nodes files chmod node-01 0750 /srv/app -R
```

**Output**

Shape from the server's schema; not captured on a live system.

`sample` holds up to 50 changed entries with their mode before and after.

```json
{
  "changed": 2,
  "dry_run": true,
  "path": "/srv/app",
  "sample": [
    { "after": "0750", "before": "0755", "path": "/srv/app" },
    { "after": "0750", "before": "0755", "path": "/srv/app/bin" }
  ],
  "skipped": 0,
  "unchanged": 14
}
```

With `-o json` the same object is under `result`, next to
`"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `2`: `<mode>` is not octal, a container root or Windows node, an invalid path, more than 200,000 entries, or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, a read-only root, or a protected path.
- `5`: MFA needed.
- `6`: the node, root or path does not exist.
- `8`: the node did not answer.

**See also**: [`forge nodes files chown`](#forge-nodes-files-chown), [`forge nodes files acl`](#forge-nodes-files-acl), [Change permissions and owners](/docs/guides/manage-files/#change-permissions-and-owners-linux), [Permissions and owners](/docs/concepts/file-explorer/#permissions-and-owners).

### forge nodes files chown

Sets the owner, the group or both, by name, on a file or directory, and with
`-R` on everything inside it. Needs `file.read` and `file.admin` on the
root, with an MFA-verified session (**Platform admin** and **Admin** roles).
The same platform limits and skip rules as [`chmod`](#forge-nodes-files-chmod)
apply.

`<spec>` is `OWNER`, `OWNER:GROUP` or `:GROUP`, for example `www-data`,
`www-data:www-data` or `:adm`.

**Synopsis**

```text
forge nodes files chown [OPTIONS] <NODE> <SPEC> <PATH>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<spec>` | string | — | yes | `OWNER`, `OWNER:GROUP` or `:GROUP` | `www-data:www-data` |
| `<path>` | path | — | yes | The file or directory | `/srv/app/uploads` |
| `--root` | string (root id) | `node` | no | The root the path is in | `--root <root-id>` |
| `-R`, `--recursive` | flag | off | no | Also everything inside folders | `-R` |
| `--dry-run` | flag | off | no | Only count what would change | `--dry-run` |

**Examples**

```sh
# Give a folder to www-data
forge nodes files chown node-01 www-data:www-data /srv/app/uploads

# Preview changing the group of a tree, then apply it
forge nodes files chown node-01 :adm /var/log/app -R --dry-run
forge nodes files chown node-01 :adm /var/log/app -R
```

**Output**

Shape from the server's schema; not captured on a live system.

`before` and `after` are `owner:group`.

```json
{
  "changed": 1,
  "dry_run": false,
  "path": "/srv/app/uploads",
  "sample": [
    { "after": "www-data:www-data", "before": "root:root", "path": "/srv/app/uploads" }
  ],
  "skipped": 0,
  "unchanged": 0
}
```

With `-o json` the same object is under `result`, next to
`"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `2`: an empty `<spec>` (`:`), an unknown user or group, a container root or Windows node, an invalid path, or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, a read-only root, or a protected path.
- `5`: MFA needed.
- `6`: the node, root or path does not exist.
- `8`: the node did not answer.

**See also**: [`forge nodes files chmod`](#forge-nodes-files-chmod), [Change permissions and owners](/docs/guides/manage-files/#change-permissions-and-owners-linux).

### forge nodes files acl

Shows who may access a path: the Windows ACL as `icacls` prints it, or the
POSIX mode on Linux. Read-only; extended Linux ACLs are not shown. Needs
`file.read` on the root.

**Synopsis**

```text
forge nodes files acl [OPTIONS] <NODE> <PATH>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<path>` | path | — | yes | The file or directory | `/etc/hostname` |
| `--root` | string (root id) | `node` | no | The root the path is in | `--root <root-id>` |

**Examples**

```sh
# Show the mode of a file on a Linux node
forge nodes files acl node-01 /etc/hostname

# Show the ACL of a folder on a Windows node
forge nodes files acl <windows-node> /C:/inetpub/wwwroot
```

**Output**

The table output prints the `acl` text only.

```text
$ forge nodes files acl node-01 /etc/hostname
POSIX permissions 0644 (extended ACLs are not shown)
```

```json
$ forge -o json nodes files acl node-01 /etc/hostname
{
  "result": {
    "acl": "POSIX permissions 0644 (extended ACLs are not shown)",
    "mode": "0644",
    "path": "/etc/hostname"
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: success.
- `2`: invalid path or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, or a protected path.
- `6`: the node, root or path does not exist.
- `8`: the node did not answer.

**See also**: [`forge nodes files chmod`](#forge-nodes-files-chmod), [`forge nodes files stat`](#forge-nodes-files-stat).

### forge nodes files compress

Packs 1–500 files and folders into a new archive on the node. The format
follows the name of `<dest>`: `.zip`, `.tar.gz` or `.tgz`. It never
overwrites an existing file. Needs `file.read` and `file.write` on the root,
with an MFA-verified session. Links, special files and protected paths are
skipped and listed in `skipped`.

**Synopsis**

```text
forge nodes files compress [OPTIONS] <NODE> <DEST> <SOURCES>...
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<dest>` | path | — | yes | The archive to create; the format follows the name | `/srv/backup/app.tar.gz` |
| `<sources>` | paths, 1–500, space-separated | — | yes | The files and folders to pack | `/srv/app/config /srv/app/public` |
| `--root` | string (root id) | `node` | no | The root all paths are in | `--root <root-id>` |

**Examples**

```sh
# Pack a folder into a zip file
forge nodes files compress node-01 /srv/backup/app.zip /srv/app

# Pack two folders into a gzip-compressed tar
forge nodes files compress node-01 /srv/backup/app.tar.gz /srv/app/config /srv/app/public
```

**Output**

Shape from the server's schema; not captured on a live system.

`size` is the archive's size; `bytes` is the content packed.

```json
{
  "bytes": 1048576,
  "dest": "/srv/backup/app.tar.gz",
  "entries": 42,
  "size": 312480,
  "skipped": [
    { "path": "/srv/app/public/current", "reason": "link" }
  ]
}
```

With `-o json` the same object is under `result`, next to
`"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `2`: `<dest>` does not end in `.zip`, `.tar.gz` or `.tgz`, more than 500 sources, an invalid path, or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, a read-only root, or a protected path.
- `5`: MFA needed.
- `6`: the node, root or a source does not exist.
- `7`: `<dest>` exists.
- `8`: the node did not answer.

**See also**: [`forge nodes files extract`](#forge-nodes-files-extract), [`forge nodes files archive`](#forge-nodes-files-archive), [Compress and extract](/docs/guides/manage-files/#compress-and-extract).

### forge nodes files extract

Unpacks a `.zip`, `.tar`, `.tar.gz` or `.tgz` into a new folder on the node.
Needs `file.read` and `file.write` on the root, with an MFA-verified
session.

The node refuses unsafe archives: entry names with absolute paths, drive
letters or `..`, more than 20,000 entries, more than 4 GiB of content, or
content more than 200 times the archive's size. Links, hard links and device
files are skipped and listed in `skipped`. Files get mode `0644` and folders
`0755`. A refused archive leaves nothing behind. See
[Archives](/docs/concepts/file-explorer/#archives).

**Synopsis**

```text
forge nodes files extract [OPTIONS] <NODE> <ARCHIVE> <DEST>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<archive>` | path | — | yes | The archive to unpack | `/srv/uploads/site.zip` |
| `<dest>` | path | — | yes | The folder to create. It must not exist, unless you pass `--replace` | `/srv/app/site` |
| `--root` | string (root id) | `node` | no | The root both paths are in | `--root <root-id>` |
| `--replace` | flag | off | no | Replace an existing folder atomically: unpack into a temporary folder, then swap it in, so the old content is served until the swap. Needs `file.delete`. Without it, an existing folder exits with `7` and the error suggests `--replace` | `--replace` |

**Examples**

```sh
# Unpack an uploaded site into a new folder
forge nodes files extract node-01 /srv/uploads/site.zip /srv/app/site

# Unpack a tarball inside an approved root
forge nodes files extract node-01 /release.tgz /release --root <root-id>
```

**Output**

Shape from the server's schema; not captured on a live system.

```json
{
  "bytes": 5242880,
  "dest": "/srv/app/site",
  "entries": 120,
  "skipped": [
    { "entry": "site/latest", "reason": "link" }
  ]
}
```

With `-o json` the same object is under `result`, next to
`"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `2`: an unsafe or unsupported archive, an invalid path, or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, a read-only root, or a protected path.
- `5`: MFA needed.
- `6`: the node, root or archive does not exist.
- `7`: `<dest>` exists.
- `8`: the node did not answer.

**See also**: [`forge nodes files compress`](#forge-nodes-files-compress), [`forge nodes files upload`](#forge-nodes-files-upload), [Compress and extract](/docs/guides/manage-files/#compress-and-extract).

### forge nodes files archive

Downloads 1–500 entries as one archive. The node packs them into a temporary
archive in Forge's private store, the CLI downloads it, and the node removes
it. Needs `file.read` on the root.

The CLI never replaces an existing local file unless you pass `--force`, and
writes through a `.forge-part` file next to `<local>`.

**Synopsis**

```text
forge nodes files archive [OPTIONS] <NODE> <LOCAL> <PATHS>...
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<local>` | local path | — | yes | Where to save the archive | `./logs.zip` |
| `<paths>` | paths, 1–500, space-separated | — | yes | The entries to pack | `/var/log/nginx /var/log/dpkg.log` |
| `--root` | string (root id) | `node` | no | The root all paths are in | `--root <root-id>` |
| `--format` | enum: `zip`, `tar_gz` | `zip` | no | The archive format | `--format tar_gz` |
| `--force` | flag | off | no | Replace `<local>` if it exists | `--force` |

**Examples**

```sh
# Download two log locations as one zip file
forge nodes files archive node-01 ./logs.zip /var/log/nginx /var/log/dpkg.log

# Download a folder as a .tar.gz, replacing an older download
forge nodes files archive node-01 ./app.tar.gz /srv/app --format tar_gz --force
```

**Output**

Shape from the server's schema; not captured on a live system.

```json
{
  "bytes": 524288,
  "entries": 18,
  "file": "./logs.zip"
}
```

`bytes` is the size of the downloaded archive. With `-o json` the same
object is under `result`, next to `"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `2`: `--format` is not `zip` or `tar_gz`, more than 500 paths, an invalid path, or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, or a protected path.
- `6`: the node, root or a path does not exist.
- `7`: `<local>` exists and `--force` is missing.
- `8`: the node did not answer.
- `9`: the transfer broke off.

**See also**: [`forge nodes files download`](#forge-nodes-files-download), [`forge nodes files compress`](#forge-nodes-files-compress).

### forge nodes files trash list

Lists the entries in the node's trash for one root. Needs `file.read` on the
root. The trash keeps entries for 30 days, up to 10 GiB per node; the oldest
go first.

**Synopsis**

```text
forge nodes files trash list [OPTIONS] <NODE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `--root` | string (root id) | `node` | no | The root whose trash to list | `--root <root-id>` |

**Examples**

```sh
# List the trash of the node root
forge nodes files trash list node-01

# List the trash of an approved root
forge nodes files trash list node-01 --root <root-id>
```

**Output**

The table shows `ID`, `PATH`, `KIND` and `SIZE`. The capture was taken with
an empty trash.

```text
$ forge nodes files trash list node-01
ID  PATH  KIND  SIZE
```

```json
$ forge -o json nodes files trash list node-01
{
  "result": [],
  "schema": "forge.cli/v1"
}
```

Each entry is `{id, path, deleted_at, size, kind}`; `deleted_at` is in epoch
milliseconds.

Exit codes:

- `0`: success.
- `2`: ambiguous node name.
- `3`: not signed in.
- `4`: permission denied.
- `6`: the node or root does not exist.
- `8`: the node did not answer.

**See also**: [`forge nodes files trash restore`](#forge-nodes-files-trash-restore), [`forge nodes files rm`](#forge-nodes-files-rm), [Delete and restore](/docs/guides/manage-files/#delete-and-restore).

### forge nodes files trash restore

Puts one trash entry back where it was. The original folder must still
exist. Needs `file.read` and `file.write` on the root, with an MFA-verified
session. Permissions and protected paths are checked again.

**Synopsis**

```text
forge nodes files trash restore [OPTIONS] <NODE> <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<id>` | string (trash id) | — | yes | The entry, from [`trash list`](#forge-nodes-files-trash-list) | `<trash-id>` |
| `--root` | string (root id) | `node` | no | The root the entry was deleted from | `--root <root-id>` |
| `--overwrite` | flag | off | no | Replace an entry that now exists at the original path | `--overwrite` |

**Examples**

```sh
# Restore an entry to its original path
forge nodes files trash restore node-01 <trash-id>

# Restore it, replacing what is there now
forge nodes files trash restore node-01 <trash-id> --overwrite
```

**Output**

Shape from the server's schema; not captured on a live system.

```json
{
  "path": "/srv/app/old",
  "restored": true
}
```

With `-o json` the same object is under `result`, next to
`"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `2`: ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, a read-only root, or a protected path.
- `5`: MFA needed.
- `6`: the node, root, trash entry or original folder does not exist.
- `7`: an entry exists at the original path and `--overwrite` is missing.
- `8`: the node did not answer.

**See also**: [`forge nodes files trash list`](#forge-nodes-files-trash-list), [`forge nodes files trash purge`](#forge-nodes-files-trash-purge).

### forge nodes files trash purge

Deletes one trash entry for good with `--id`, or every entry of the root's
trash without it. Needs `file.read` and `file.delete` on the root, with an
MFA-verified session. The command asks first unless you pass `--yes`.

:::caution
Without `--id` the command empties the whole trash of the root. Purged
entries cannot be restored.
:::

**Synopsis**

```text
forge nodes files trash purge [OPTIONS] <NODE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `--id` | string (trash id) | — (all entries) | no | The one entry to purge | `--id <trash-id>` |
| `--root` | string (root id) | `node` | no | The root whose trash to purge | `--root <root-id>` |
| `--yes` | flag | off | no | Do not ask first. Required with `--non-interactive` or without a terminal | `--yes` |

**Examples**

```sh
# Purge one entry
forge nodes files trash purge node-01 --id <trash-id>

# Empty the trash of an approved root without asking
forge nodes files trash purge node-01 --root <root-id> --yes
```

**Output**

Shape from the server's schema; not captured on a live system.

`purged` is the number of entries removed.

```json
{
  "purged": 3
}
```

With `-o json` the same object is under `result`, next to
`"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `1`: you did not answer `yes` at the prompt.
- `2`: no `--yes` where no prompt is possible, or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied, or a read-only root.
- `5`: MFA needed.
- `6`: the node, root or trash entry does not exist.
- `8`: the node did not answer.

**See also**: [`forge nodes files trash list`](#forge-nodes-files-trash-list), [Trash and revisions](/docs/concepts/file-explorer/#trash-and-revisions).

### forge nodes files sftp info

Shows where SFTP clients connect: the gateway's host, port and host key
fingerprint, and the longest access you can issue (`max_hours`). It contains
no secrets. Needs `file.read` on the node. When the gateway is off, the
answer is only `{"enabled": false}`.

**Synopsis**

```text
forge nodes files sftp info [OPTIONS] <NODE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |

**Examples**

```sh
# Show the gateway details
forge nodes files sftp info node-01

# Print the fingerprint to compare with your SFTP client
forge -o json nodes files sftp info node-01 | jq -r '.result.fingerprint'
```

**Output**

```text
$ forge nodes files sftp info node-01
{
  "enabled": true,
  "fingerprint": "SHA256:<host-key-fingerprint>",
  "host": "192.0.2.10",
  "max_hours": 168,
  "port": 2022,
  "protocol": "SFTP"
}
```

```json
$ forge -o json nodes files sftp info node-01
{
  "result": {
    "enabled": true,
    "fingerprint": "SHA256:<host-key-fingerprint>",
    "host": "192.0.2.10",
    "max_hours": 168,
    "port": 2022,
    "protocol": "SFTP"
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: success, also when the gateway is off.
- `2`: ambiguous node name.
- `3`: not signed in.
- `4`: permission denied.
- `6`: the node does not exist.

**See also**: [`forge nodes files sftp grant`](#forge-nodes-files-sftp-grant), [Give temporary SFTP access](/docs/guides/sftp-access/), [SFTP gateway configuration](/docs/reference/configuration/#sftp-gateway).

### forge nodes files sftp list

Lists temporary access grants. Without a node it lists every node you may
see; with a node, only that node. On each node where you hold `file.read`
you see every grant with `file.admin`, otherwise only your own. Nodes you
cannot see are left out. Passwords and their hashes are never shown.

**Synopsis**

```text
forge nodes files sftp list [OPTIONS] [NODE]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | every node | no | Only grants on this node | `node-01` |
| `--status` | `active`, `expired` or `revoked` | any | no | Only grants with this status | `--status active` |
| `--search` | string | — | no | Case-insensitive match on the name, grant id, creator or node name | `--search vendor` |

**Examples**

```sh
# Every grant you may see, on every node
forge nodes files sftp list

# Active grants on node-01
forge nodes files sftp list node-01 --status active

# Grants that never expire, as JSON
forge -o json nodes files sftp list | jq '.result[] | select(.never_expires)'
```

**Output**

Shape from the server's schema; not captured on a live system.

The table shows `ID`, `NAME`, `NODE`, `STATUS`, `WRITE`, `DELETE`, `NEVER`
(never expires) and `ROOTS`.

```text
$ forge nodes files sftp list --status active
ID                  NAME           NODE     STATUS  WRITE  DELETE  NEVER  ROOTS
fg<16-hex-digits>   Vendor upload  node-01  active  true   false   false  ["<root-id>"]
```

```json
$ forge -o json nodes files sftp list --search vendor
{
  "result": [
    {
      "active": true,
      "allowed_ips": [],
      "bandwidth_kbps": null,
      "bytes_transferred": 1048576,
      "created_at": 1794556400000,
      "created_by": "user:<user-id>",
      "delete": false,
      "expires_at": 1794560000000,
      "id": "fg<16-hex-digits>",
      "last_used_at": 1794557000000,
      "max_sessions": 1,
      "name": "Vendor upload",
      "never_expires": false,
      "revoked_at": null,
      "roots": ["<root-id>"],
      "server_id": "<server-id>",
      "server_name": "node-01",
      "status": "active",
      "transfer_limit": null,
      "workspace_id": "<workspace-id>",
      "write": true
    }
  ],
  "schema": "forge.cli/v1"
}
```

Each grant has the fields shown under [`sftp grant`](#forge-nodes-files-sftp-grant),
plus `active` (usable now), `status` (`active`, `expired` or `revoked`),
`never_expires` and `server_name`.

Exit codes:

- `0`: success.
- `2`: an invalid `--status`, or ambiguous node name.
- `3`: not signed in.
- `4`: permission denied.
- `6`: the node does not exist.

**See also**: [`forge nodes files sftp get`](#forge-nodes-files-sftp-get), [`forge nodes files sftp revoke`](#forge-nodes-files-sftp-revoke), [Give temporary SFTP access](/docs/guides/sftp-access/).

### forge nodes files sftp get

Shows one grant. Visible to the same people as in
[`sftp list`](#forge-nodes-files-sftp-list): its issuer, or anyone with
`file.admin` on the node. The password and its hash are never shown.

**Synopsis**

```text
forge nodes files sftp get [OPTIONS] <NODE> <GRANT>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<grant>` | string (grant id) | — | yes | The grant | `fg<16-hex-digits>` |

**Examples**

```sh
# Show a grant
forge nodes files sftp get node-01 <grant-id>

# Print when it expires
forge -o json nodes files sftp get node-01 <grant-id> | jq '.result.expires_at'
```

**Output**

Shape from the server's schema; not captured on a live system.

```json
$ forge nodes files sftp get node-01 fg<16-hex-digits>
{
  "active": true,
  "allowed_ips": [],
  "bandwidth_kbps": null,
  "bytes_transferred": 1048576,
  "created_at": 1794556400000,
  "created_by": "user:<user-id>",
  "delete": false,
  "expires_at": 1794563600000,
  "id": "fg<16-hex-digits>",
  "last_used_at": 1794557000000,
  "max_sessions": 1,
  "name": "Vendor upload",
  "never_expires": false,
  "revoked_at": null,
  "roots": ["<root-id>"],
  "server_id": "<server-id>",
  "status": "active",
  "transfer_limit": null,
  "workspace_id": "<workspace-id>",
  "write": false
}
```

With `-o json` the same object is under `result`, next to
`"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `2`: ambiguous node name.
- `3`: not signed in.
- `4`: permission denied.
- `6`: the node or grant does not exist, or is not yours to see.

**See also**: [`forge nodes files sftp update`](#forge-nodes-files-sftp-update).

### forge nodes files sftp grant

Issues temporary SFTP access to one or more roots of the node and prints the
connection details. The user name is the grant id (`fg` and 16 hex digits);
the password is shown once. Needs an MFA-verified session and every right
the access carries: `file.read`, plus `file.write` with `--write` and
`file.delete` with `--delete`. The `node` root also needs `file.node`. The
SFTP gateway must be on.

The access can never do more than you can. If you lose a right, the access
loses it too. Read-only roots accept only read-only access.

:::caution
The password is shown once and Forge does not store it. Anyone with the user
name and password can use the access until it expires or you revoke it.
Send the password through a separate channel, give the shortest lifetime
that works, and restrict `--allow-ip` when you know the client's address.
:::

**Synopsis**

```text
forge nodes files sftp grant [OPTIONS] --name <NAME> <NODE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `--name` | string, 1–120 characters | — | yes | A label for the person or tool | `--name "Vendor upload"` |
| `--root` | string (root id), repeatable, 1–8 | `node` | no | Roots to expose | `--root <root-id>` |
| `--write` | flag | off | no | Allow uploads, new folders and renames (`file.write`) | `--write` |
| `--delete` | flag | off | no | Allow deletes and replacing files (`file.delete`) | `--delete` |
| `--minutes` | integer (minutes) | `60` | no | Lifetime, 1 up to `max_hours` × 60 from [`sftp info`](#forge-nodes-files-sftp-info) | `--minutes 30` |
| `--never` | flag | off | no | Never expire. Refused unless `[sftp] allow_never_expiry` is on and you hold `file.admin`; `--minutes` is then ignored | `--never` |
| `--allow-ip` | string (IP or CIDR), repeatable, up to 32 | any address | no | Client addresses allowed to connect | `--allow-ip 198.51.100.0/24` |
| `--max-sessions` | integer, 1–10 | `1` | no | Sessions at once | `--max-sessions 2` |

**Examples**

```sh
# Read-only access to the whole node for one hour
forge nodes files sftp grant node-01 --name "Log review"

# Upload access to one approved root for 30 minutes
forge nodes files sftp grant node-01 --name "Vendor upload" --root <root-id> --write --minutes 30

# Upload and delete access from one network, two sessions
forge nodes files sftp grant node-01 --name "Migration" --root <root-id> --write --delete \
  --allow-ip 198.51.100.0/24 --max-sessions 2
```

**Output**

Shape from the server's schema; not captured on a live system.

The table output prints the connection details. A note on stderr says the
password is shown only once.

```text
host        192.0.2.10
port        2022
username    fg<16-hex-digits>
password    <password>
fingerprint SHA256:<host-key-fingerprint>
```

With `-o json` you get the grant and the connection:

```json
{
  "result": {
    "connection": {
      "expires_at": 1794560000000,
      "fingerprint": "SHA256:<host-key-fingerprint>",
      "host": "192.0.2.10",
      "password": "<password>",
      "port": 2022,
      "protocol": "SFTP",
      "username": "fg<16-hex-digits>"
    },
    "grant": {
      "allowed_ips": [],
      "bandwidth_kbps": null,
      "bytes_transferred": 0,
      "created_at": 1794556400000,
      "created_by": "user:<user-id>",
      "delete": false,
      "expires_at": 1794560000000,
      "id": "fg<16-hex-digits>",
      "last_used_at": null,
      "max_sessions": 1,
      "name": "Vendor upload",
      "revoked_at": null,
      "roots": ["<root-id>"],
      "server_id": "<server-id>",
      "transfer_limit": null,
      "workspace_id": "<workspace-id>",
      "write": true
    }
  },
  "schema": "forge.cli/v1"
}
```

Times are in epoch milliseconds.

Exit codes:

- `0`: success.
- `2`: an invalid name, root count, lifetime, address or session count; a read-only root with `--write` or `--delete`; or ambiguous node name.
- `3`: not signed in.
- `4`: you do not hold a right the access would carry.
- `5`: MFA needed.
- `6`: the node or a root does not exist.
- `7`: the SFTP gateway is off (`sftp_disabled`).

**See also**: [`forge nodes files sftp revoke`](#forge-nodes-files-sftp-revoke), [Give temporary SFTP access](/docs/guides/sftp-access/), [Temporary SFTP access](/docs/concepts/file-explorer/#temporary-sftp-access).

### forge nodes files sftp revoke

Revokes a grant. Open sessions close within seconds. The issuer can revoke
their own grant; anyone else needs `file.admin` on the node, with an
MFA-verified session. The command asks first unless you pass `--yes`.
Revoking a grant that is already revoked changes nothing.

**Synopsis**

```text
forge nodes files sftp revoke [OPTIONS] <NODE> <GRANT>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<grant>` | string (grant id) | — | yes | The grant, from [`sftp list`](#forge-nodes-files-sftp-list) | `fg<16-hex-digits>` |
| `--yes` | flag | off | no | Do not ask first. Required with `--non-interactive` or without a terminal | `--yes` |

**Examples**

```sh
# Revoke a grant (asks first)
forge nodes files sftp revoke node-01 <grant-id>

# Revoke it from a script
forge nodes files sftp revoke node-01 <grant-id> --yes
```

**Output**

Shape from the server's schema; not captured on a live system.

The revoked grant, with `revoked_at` set:

```json
{
  "allowed_ips": [],
  "bandwidth_kbps": null,
  "bytes_transferred": 1048576,
  "created_at": 1794556400000,
  "created_by": "user:<user-id>",
  "delete": false,
  "expires_at": 1794560000000,
  "id": "fg<16-hex-digits>",
  "last_used_at": 1794557000000,
  "max_sessions": 1,
  "name": "Vendor upload",
  "revoked_at": 1794558000000,
  "roots": ["<root-id>"],
  "server_id": "<server-id>",
  "transfer_limit": null,
  "workspace_id": "<workspace-id>",
  "write": true
}
```

With `-o json` the same object is under `result`, next to
`"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `1`: you did not answer `yes` at the prompt.
- `2`: no `--yes` where no prompt is possible, or ambiguous node name.
- `3`: not signed in.
- `4`: the grant is someone else's and you lack `file.admin`.
- `5`: MFA needed.
- `6`: the node or grant does not exist.

**See also**: [`forge nodes files sftp list`](#forge-nodes-files-sftp-list), [Revoke access](/docs/guides/sftp-access/#revoke-access).

### forge nodes files sftp update

Changes a grant without changing its password: name, roots, permissions,
expiry and restrictions. Only the options you pass change. The rules are
the same as for [`sftp grant`](#forge-nodes-files-sftp-grant): an
MFA-verified session, every right the access carries, and the issuer or
`file.admin` on the node. Revoked grants cannot be changed.

Changes apply to new sessions at once. When a change takes something away
(a root, upload or delete, a client address, a session, bandwidth, transfer
allowance or lifetime), Forge closes the grant's open sessions so clients
reconnect with the new settings. Every SFTP operation also re-reads the
grant.

**Synopsis**

```text
forge nodes files sftp update [OPTIONS] <NODE> <GRANT>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<grant>` | string (grant id) | — | yes | The grant | `fg<16-hex-digits>` |
| `--name` | string, 1–120 characters | unchanged | no | New label | `--name "Vendor upload (May)"` |
| `--root` | string (root id), repeatable, 1–8 | unchanged | no | Replaces the roots. Only roots you can open now | `--root <root-id>` |
| `--write` | `true` or `false` | unchanged | no | Allow uploads, new folders and renames (`file.write`) | `--write false` |
| `--delete` | `true` or `false` | unchanged | no | Allow deletes and replacing files (`file.delete`) | `--delete true` |
| `--minutes` | integer (minutes) | unchanged | no | New lifetime counted from now, 1 up to `max_hours` × 60. Cannot be combined with `--never` | `--minutes 120` |
| `--never` | flag | off | no | Never expire. Refused unless `[sftp] allow_never_expiry` is on and you hold `file.admin` | `--never` |
| `--allow-ip` | string (IP or CIDR), repeatable, up to 32 | unchanged | no | Replaces the allowed client addresses. Cannot be combined with `--any-ip` | `--allow-ip 198.51.100.0/24` |
| `--any-ip` | flag | off | no | Allow any client address | `--any-ip` |
| `--max-sessions` | integer, 1–10 | unchanged | no | Sessions at once | `--max-sessions 2` |
| `--bandwidth-kbps` | integer (KiB/s) | unchanged | no | Bandwidth cap per session | `--bandwidth-kbps 2048` |
| `--no-bandwidth-limit` | flag | off | no | Remove the bandwidth cap | `--no-bandwidth-limit` |
| `--transfer-limit` | integer (bytes) | unchanged | no | Total bytes the access may transfer | `--transfer-limit 1073741824` |
| `--no-transfer-limit` | flag | off | no | Remove the transfer allowance | `--no-transfer-limit` |

**Examples**

```sh
# Make a grant read-only (open sessions close)
forge nodes files sftp update node-01 <grant-id> --write false --delete false

# Extend it by two hours from now
forge nodes files sftp update node-01 <grant-id> --minutes 120

# Restrict it to one network and 1 GiB in total
forge nodes files sftp update node-01 <grant-id> --allow-ip 198.51.100.0/24 --transfer-limit 1073741824
```

**Output**

Shape from the server's schema; not captured on a live system.

The changed grant:

```json
$ forge nodes files sftp update node-01 fg<16-hex-digits> --write false --minutes 120
{
  "active": true,
  "allowed_ips": [],
  "bandwidth_kbps": null,
  "bytes_transferred": 1048576,
  "created_at": 1794556400000,
  "created_by": "user:<user-id>",
  "delete": false,
  "expires_at": 1794563600000,
  "id": "fg<16-hex-digits>",
  "last_used_at": 1794557000000,
  "max_sessions": 1,
  "name": "Vendor upload",
  "never_expires": false,
  "revoked_at": null,
  "roots": ["<root-id>"],
  "server_id": "<server-id>",
  "status": "active",
  "transfer_limit": null,
  "workspace_id": "<workspace-id>",
  "write": false
}
```

With `-o json` the same object is under `result`, next to
`"schema": "forge.cli/v1"`.

Exit codes:

- `0`: success.
- `2`: nothing to change, conflicting options, an invalid value, a read-only root with write or delete, or ambiguous node name.
- `3`: not signed in.
- `4`: the grant is someone else's and you lack `file.admin`, you do not hold a right the access would carry, or `--never` is not allowed.
- `5`: MFA needed.
- `6`: the node, grant or a root does not exist.
- `7`: the grant is revoked.

**See also**: [`forge nodes files sftp reset-password`](#forge-nodes-files-sftp-reset-password), [Change access](/docs/guides/sftp-access/#change-access).

### forge nodes files sftp reset-password

Replaces a grant's password and prints the new one once. The old password
stops working and the grant's open sessions close. Needs an MFA-verified
session, and the issuer or `file.admin` on the node. The command asks first
unless you pass `--yes`.

**Synopsis**

```text
forge nodes files sftp reset-password [OPTIONS] <NODE> <GRANT>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<node>` | node id, host name or display name | — | yes | The node | `node-01` |
| `<grant>` | string (grant id) | — | yes | The grant | `fg<16-hex-digits>` |
| `--yes` | flag | off | no | Do not ask first. Required with `--non-interactive` or without a terminal | `--yes` |

**Examples**

```sh
# Reset the password (asks first)
forge nodes files sftp reset-password node-01 <grant-id>

# From a script, keeping only the password
forge -o json nodes files sftp reset-password node-01 <grant-id> --yes | jq -r '.result.connection.password'
```

**Output**

Shape from the server's schema; not captured on a live system.

The table output prints the connection details, as
[`sftp grant`](#forge-nodes-files-sftp-grant) does. A note on stderr says
the password is shown only once.

```text
$ forge nodes files sftp reset-password node-01 fg<16-hex-digits> --yes
host        192.0.2.10
port        2022
username    fg<16-hex-digits>
password    <password>
fingerprint SHA256:<host-key-fingerprint>
```

With `-o json` you get `{grant, connection}` under `result`, like
[`sftp grant`](#forge-nodes-files-sftp-grant).

Exit codes:

- `0`: success.
- `1`: you did not answer `yes` at the prompt.
- `2`: no `--yes` where no prompt is possible, or ambiguous node name.
- `3`: not signed in.
- `4`: the grant is someone else's and you lack `file.admin`.
- `5`: MFA needed.
- `6`: the node or grant does not exist.
- `7`: the grant is revoked, or the SFTP gateway is off.

**See also**: [`forge nodes files sftp update`](#forge-nodes-files-sftp-update).
