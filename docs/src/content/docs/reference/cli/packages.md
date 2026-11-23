---
title: forge packages
description: Browse the App Store catalog, install packages on nodes, run lifecycle actions, and check, update and set the update policy of installations.
sidebar:
  order: 24
---

`forge packages` drives the App Store: the catalog of native packages
(runtimes, web servers, databases and more), what is installed on which
node, and package updates. For the tasks behind these commands, see
[App Store](/docs/guides/app-store/).
Every command takes the [global flags](/docs/reference/cli/#global-flags).

## Commands

| Command | What it does |
| --- | --- |
| [`forge packages catalog`](#forge-packages-catalog) | Lists the packages you can install |
| [`forge packages installed`](#forge-packages-installed) | Lists installations on the nodes you can see |
| [`forge packages install`](#forge-packages-install) | Installs a package on one or more nodes |
| [`forge packages updates`](#forge-packages-updates) | Lists installed, available and running builds per installation |
| [`forge packages check`](#forge-packages-check) | Asks the nodes for fresh build information now |
| [`forge packages update`](#forge-packages-update) | Updates one installation to its candidate build, or a build you name |
| [`forge packages policy`](#forge-packages-policy) | Sets an installation's update policy |
| [`forge packages action`](#forge-packages-action) | Starts, stops, restarts, checks or uninstalls an installation |
| [`forge packages releases`](#forge-packages-releases) | Lists vendor release lines, LTS and end-of-life dates |

Where a command takes `<installation>`, give the installation's id from
[`forge packages installed`](#forge-packages-installed). Names are not
accepted.

### forge packages catalog

Lists the packages of the App Store catalog, with their versions,
parameters, supported operating systems and how they integrate with Forge.
Needs only a signed-in session.

**Synopsis**

```text
forge packages catalog [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

**Examples**

```sh
# Every package, as a table
forge packages catalog
# Versions and parameters of one package
forge -o json packages catalog | jq '.result.packages[] | select(.id == "redis") | {versions, params}'
```

**Output**

```text
$ forge packages catalog
ID             NAME                    CATEGORY    KIND       OS             SUMMARY
php            PHP-FPM                 runtimes    native     linux          PHP with FPM for Nginx; several versions can be installed s…
python         Python                  runtimes    native     linux          Python 3 with venv and pip for WSGI/ASGI apps (gunicorn, uv…
nodejs         Node.js                 runtimes    native     linux,windows  Node.js LTS from NodeSource.
nginx          Nginx                   web         native     linux          Web server and reverse proxy; turns a node into an edge nod…
postgresql     PostgreSQL              databases   native     linux,windows  The PostgreSQL server from the official PGDG repository.
…
```

```json
$ forge -o json packages catalog
{
  "result": {
    "packages": [
      {
        "category": "runtimes",
        "homepage": "https://www.php.net",
        "id": "php",
        "integration": { "exports": [ … ], "paths": [ … ], "role": "…", "services": [ … ], "usage": [ … ], "used_by": [ … ] },
        "kind": "native",
        "license": "PHP-3.01",
        "name": "PHP-FPM",
        "notes": "…",
        "os": ["linux"],
        "params": [
          {
            "default": "mbstring,xml,curl,zip,intl,gd,mysql,pgsql,redis,opcache",
            "help": "Comma-separated PHP extension names.",
            "key": "extensions",
            "kind": { "type": "list" },
            "label": "Extensions"
            …
```

The table shows `result.packages`. The JSON also holds `roadmap`: packages
planned for later, each with a `name` and `description`. A package's
`versions` lists the versions you can install; the first is the default.
`requires` lists packages that must be installed on the same node first.

Exit codes: `0` success; `3` not signed in; `9` server unreachable.

**See also**: [`forge packages install`](#forge-packages-install),
[Catalog](/docs/guides/app-store/#catalog).

### forge packages installed

Lists the installations on the nodes you can see: package, node, version,
lifecycle status, service state and update policy. Needs `view` on each node; other
nodes' installations are left out.

**Synopsis**

```text
forge packages installed [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--node <NODE>` | node id, host name or display name | none | no | Only this node's installations (the API's `server_id` filter) | `--node web-01` |
| `--package <PACKAGE>` | catalog id | none | no | Only this package | `--package postgres` |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

**Examples**

```sh
# Every installation
forge packages installed
# Installations that are not running
forge -o json packages installed | jq -r '.result[] | select(.state != "running") | "\(.id) \(.package)"'
```

**Output**

```text
$ forge packages installed
ID                                    PACKAGE   NODE   VERSION  STATUS      STATE    POLICY
01900000-0000-7000-8000-000000000041  docker    lab-c  stable   installed   running  notify
01900000-0000-7000-8000-000000000042  docker    lab-b  stable   installed   running  notify
01900000-0000-7000-8000-000000000043  nginx     lab-b  stable   installed   stopped  notify
01900000-0000-7000-8000-000000000045  postgres  lab-c  17       installing  -        notify
01900000-0000-7000-8000-000000000046  redis     lab-c  stable   failed      -        notify
```

| Column | JSON field | Meaning |
| --- | --- | --- |
| `STATUS` | `status` | The installation's lifecycle in Forge: `installed`, `failed`, or a busy state ending in `ing`, such as `installing`, `updating` or `uninstalling` |
| `STATE` | `state` | The service state the last job reported from the node: `running` or `stopped`. `-` until a job has reported one |
| `NODE` | `node` | The node's host name, else its display name. The node's id when you cannot list nodes |

An installation can be `installed` and `stopped` at once: the package is on
the node and its service does not run. Refresh `STATE` with
[`forge packages action <installation> status`](#forge-packages-action).

```json
$ forge -o json packages installed
{
  "result": [
    {
      "created_at": 1794554351309,
      "env": {
        "FORGE_PKG_DOCKER_BIN": "/usr/bin/docker",
        "FORGE_PKG_DOCKER_SOCKET": "/var/run/docker.sock",
        "FORGE_PKG_DOCKER_VERSION": "stable"
      },
      "id": "01900000-0000-7000-8000-000000000041",
      "last_error": null,
      "last_job_id": "01900000-0000-7000-8000-000000000044",
      "package": "docker",
      "params": {},
      "server_id": "01900000-0000-7000-8000-000000000006",
      "state": "running",
      "status": "installed",
      "update_policy": "notify",
      "updated_at": 1794589954362,
      "version": "stable",
      "versions": {
        "candidate": "5:29.9.3-1~debian.13~trixie",
        "checked_at": 1794589954361,
        "installed": "5:29.9.3-1~debian.13~trixie",
        "manager": "apt",
        "package": "docker-ce",
        "running": "29.9.3"
      },
      "workspace_id": "01900000-0000-7000-8000-000000000001"
    },
    …
  ],
  "schema": "forge.cli/v1"
}
```

In the JSON output, `server_id` is the node's id; the CLI adds `node`, the
name the table shows. `state` is `null` until a job has reported
a service state. `versions` is `null` until Forge has checked the node for
builds.

Exit codes: `0` success; `3` not signed in; `9` server unreachable.

**See also**: [`forge packages updates`](#forge-packages-updates),
[`forge packages action`](#forge-packages-action).

### forge packages install

Installs a package on one or more nodes. Forge checks every node first, so
the install starts everywhere or nowhere, then runs one `package.action` job
per node. Needs `package.manage` on every node, with an MFA-verified session
(**Operator** or higher). Audited as `package.install`.

**Synopsis**

```text
forge packages install [OPTIONS] --file <FILE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--file <FILE>` | path, `@<path>` or `@-` | — | yes | The install request as JSON (fields below). `@-` reads it from stdin | `--file install.json` |

The JSON body:

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `package` | string | — (required) | Package id from the catalog, such as `nginx` |
| `version` | string | — (required) | One of the package's `versions` in the catalog, such as `stable` |
| `params` | object of strings | `{}` | Values for the package's `params`, keyed by `key`. Missing keys take the parameter's default |
| `server_ids` | list of node ids | — (required) | 1 to 100 nodes. Node names are not accepted |

The CLI checks the body against the server's schema before sending and
exits with `2` (`invalid_body`) on a mismatch; see
[`forge api`](/docs/reference/cli/api/).

Example `install.json`:

```json
{
  "package": "nginx",
  "version": "stable",
  "params": {},
  "server_ids": ["<node-id>"]
}
```

:::caution
Packages install as root on the node. Installing again on a node that
already has the same package and version repairs it with the new
parameters.
:::

**Examples**

```sh
# Install from a file
forge packages install --file install.json
# Install Nginx on a node found by name, then wait for the job
NODE_ID=$(forge -o json nodes get <node-name> | jq -r '.result.id')
JOB_ID=$(jq -n --arg n "$NODE_ID" '{package: "nginx", version: "stable", server_ids: [$n]}' \
  | forge -o json packages install --file @- | jq -r '.result[0].job.id')
forge jobs wait "$JOB_ID"
```

**Output**

The table output prints the result as a key/value list. With `-o json`, one
element per node:

```json
{
  "schema": "forge.cli/v1",
  "result": [
    {
      "server_id": "<node-id>",
      "installation": { "id": "<installation-id>", "package": "nginx", "version": "stable", "status": "installing", "…": "…" },
      "job": { "id": "<job-id>", "kind": "package.action", "status": "pending", "…": "…" }
    }
  ]
}
```

Shape from the server's schema; not captured on a live system. The command
returns once the jobs are queued; follow them with
[`forge jobs wait`](/docs/reference/cli/jobs/#forge-jobs-wait).

Exit codes: `0` the jobs were queued; `2` invalid body, an unknown package,
version or parameter, an empty or too long node list, or a node whose OS the
package does not support; `3` not signed in; `4` permission denied; `5` MFA
needed; `6` no such node; `7` a required package is missing on a node, or
the package is busy there; `9` server unreachable.

**See also**: [`forge packages catalog`](#forge-packages-catalog),
[`forge jobs`](/docs/reference/cli/jobs/),
[Install a package](/docs/guides/app-store/#install-a-package).

### forge packages updates

Lists, for each installation from a package repository (apt, dnf or winget),
the installed build, the build the node's repositories offer, the version the
running service reports, and the vendor's release line. Needs `view` on each
node; other nodes' installations are left out.

**Synopsis**

```text
forge packages updates [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

**Examples**

```sh
# Installed and available builds
forge packages updates
# Installations with a newer build available
forge -o json packages updates | jq -r '.result[] | select(.kind != null) | "\(.installation_id) \(.package) \(.installed) -> \(.candidate)"'
```

**Output**

```text
$ forge packages updates
INSTALLATION                          PACKAGE  INSTALLED                    CANDIDATE                    STATE
01900000-0000-7000-8000-000000000041  docker   5:29.9.3-1~debian.13~trixie  5:29.9.3-1~debian.13~trixie  running
01900000-0000-7000-8000-000000000042  docker   5:29.9.3-1~debian.13~trixie  5:29.9.3-1~debian.13~trixie  running
01900000-0000-7000-8000-000000000043  nginx    1.26.3-3+deb13u10            1.26.3-3+deb13u10            running
```

```json
$ forge -o json packages updates
{
  "result": [
    {
      "candidate": "5:29.9.3-1~debian.13~trixie",
      "checked_at": 1794589954361,
      "installation_id": "01900000-0000-7000-8000-000000000041",
      "installed": "5:29.9.3-1~debian.13~trixie",
      "kind": null,
      "line": "stable",
      "manager": "apt",
      "name": "Docker Engine",
      "os_package": "docker-ce",
      "package": "docker",
      "release_notes": "https://docs.docker.com/engine/release-notes/",
      "running": "29.9.3",
      "server_id": "01900000-0000-7000-8000-000000000006",
      "state": "running",
      "status": "installed",
      "update_policy": "notify",
      "upstream": {
        "ahead_of_node": false,
        "eol": "false",
        "latest": "29.9.3",
        "line": "29",
        "lts": false,
        "newest_line": null,
        "support_until": null
      }
    },
    …
  ],
  "schema": "forge.cli/v1"
}
```

`kind` is the size of the available update: `revision` (same upstream
version, new package revision, such as a security backport), `patch`,
`minor` or `major`. It is `null` when the candidate is not newer than the
installed build, or before the first check. Packages installed from a vendor
tarball or script are not listed.
`upstream.ahead_of_node` is `true` when the vendor has released a newer build
than the node's repositories offer.

Exit codes: `0` success; `3` not signed in; `9` server unreachable.

**See also**: [`forge packages check`](#forge-packages-check),
[`forge packages update`](#forge-packages-update),
[Update a package](/docs/guides/app-store/#update-a-package).

### forge packages check

Asks every node you manage for fresh build information now, instead of
waiting for the six-hourly check, and refreshes the vendor release data in
the background. Only installed packages from a package repository are
checked. Needs `server.manage` on each node (**Operator** or higher); nodes
without it are skipped.

**Synopsis**

```text
forge packages check [OPTIONS]
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
# Check now
forge packages check
# Check, then list the results once the check jobs have run
forge packages check && forge packages updates
```

**Output**

The table output prints the result as a key/value list. With `-o json`:

```json
{ "schema": "forge.cli/v1", "result": { "checks": 3 } }
```

Shape from the server's schema; not captured on a live system. `checks` is
the number of check jobs queued. The jobs run in the background; `updates`
shows their results when they finish.

Exit codes: `0` the checks were queued; `3` not signed in; `9` server
unreachable.

**See also**: [`forge packages updates`](#forge-packages-updates).

### forge packages update

Updates one installation to an exact build. By default the CLI installs
the candidate build the node reported, as listed by
[`forge packages updates`](#forge-packages-updates). The confirmation names
that build: `Update installation <id> to <build>?`. Forge installs exactly
that build, keeps your configuration files and restarts the service. Needs
`package.manage` on the node, with an MFA-verified session (**Operator** or
higher). Audited as `package.update`.

**Synopsis**

```text
forge packages update [OPTIONS] <INSTALLATION>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<installation>` | installation id | — | yes | The installation to update | `<installation-id>` |
| `--build <BUILD>` | string | the node's candidate build | no | The build to install. The server accepts only the node's current candidate, so use this to pin the build you reviewed | `--build 1.26.3-3+deb13u11` |
| `--yes` | flag | off | no | Skip the confirmation prompt | `--yes` |

:::caution
The service restarts during the update. Services on the node that use
it have a short outage.
:::

**Examples**

```sh
# Update to the candidate build, with a confirmation prompt
forge packages update <installation-id>
# Check first, then update to the build you reviewed and wait for the job
forge packages check
forge -o json packages updates | jq -r '.result[] | select(.installation_id == "<installation-id>") | .candidate'
JOB_ID=$(forge -o json packages update <installation-id> --build <candidate> --yes | jq -r '.result.id')
forge jobs wait "$JOB_ID"
```

**Output**

`result` is the queued `package.action` job, as shown by
[`forge jobs get`](/docs/reference/cli/jobs/#forge-jobs-get):

```json
{
  "schema": "forge.cli/v1",
  "result": { "id": "<job-id>", "kind": "package.action", "status": "pending", "…": "…" }
}
```

Shape from the server's schema; not captured on a live system.

Exit codes: `0` the update job was queued; `1` you did not confirm; `2` an
invalid build, an invalid id, or no `--yes` where no prompt is possible; `3`
not signed in; `4` permission denied; `5` MFA needed; `6` no such
installation, or no candidate build without `--build` (`no_candidate`: run
[`forge packages check`](#forge-packages-check) first); `7` no check has run
yet, the build is not the node's candidate or is not newer than the
installed one, or the installation is busy; `9` server unreachable.

**See also**: [`forge packages updates`](#forge-packages-updates),
[`forge packages policy`](#forge-packages-policy),
[`forge jobs wait`](/docs/reference/cli/jobs/#forge-jobs-wait),
[Update a package](/docs/guides/app-store/#update-a-package).

### forge packages policy

Sets an installation's update policy. Needs `package.manage` on the node,
with an MFA-verified session. Audited as `package.update_policy`. The
command does not ask for confirmation.

**Synopsis**

```text
forge packages policy [OPTIONS] <INSTALLATION> <POLICY>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<installation>` | installation id | — | yes | The installation to change | `<installation-id>` |
| `<policy>` | enum: `notify`, `auto_patch` | — | yes | `notify`: list updates and wait for you (the default for new installations). `auto_patch`: apply security, packaging and patch updates on the next six-hourly check; minor and major updates still wait. The updates run as you, with your roles at that time | `auto_patch` |
| `--yes` | flag | off | no | Skip the confirmation prompt (needed without a terminal) | `--yes` |

**Examples**

```sh
# Apply patch updates automatically
forge packages policy <installation-id> auto_patch --yes
# Set every Nginx installation back to notify
forge -o json packages installed | jq -r '.result[] | select(.package == "nginx") | .id' \
  | xargs -I{} forge packages policy {} notify --yes
```

**Output**

The table output prints the installation as a key/value list. With `-o json`,
`result` has the shape of one element of
[`forge packages installed`](#forge-packages-installed), with the new
`update_policy`.

Shape from the server's schema; not captured on a live system.

Exit codes: `0` set; `2` a policy other than `notify` or `auto_patch`, or an
invalid id; `3` not signed in; `4` permission denied; `5` MFA needed; `6` no
such installation; `9` server unreachable.

**See also**: [`forge packages updates`](#forge-packages-updates),
[Update a package](/docs/guides/app-store/#update-a-package).

### forge packages action

Runs a lifecycle action on an installation as a `package.action` job.
`start`, `stop`, `restart`, `uninstall` and `install` need `package.manage`
on the node, with an MFA-verified session, and are audited as
`package.<action>`. `status` and `versions` only read state and need
`server.manage` on the node.

**Synopsis**

```text
forge packages action [OPTIONS] <INSTALLATION> <ACTION>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<installation>` | installation id | — | yes | The installation to act on | `<installation-id>` |
| `<action>` | enum: `start`, `stop`, `restart`, `status`, `versions`, `uninstall`, `install` | — | yes | `status` refreshes the service state; `versions` refreshes the build information; `install` runs the install again to repair it. `upgrade` is refused: use [`forge packages update`](#forge-packages-update) | `restart` |
| `--yes` | flag | off | no | Skip the confirmation prompt | `--yes` |

:::caution
`stop`, `restart` and `uninstall` interrupt the service and every
service on the node that uses it.
:::

**Examples**

```sh
# Restart the service, with a confirmation prompt
forge packages action <installation-id> restart
# Refresh the service state and wait for the job
JOB_ID=$(forge -o json packages action <installation-id> status --yes | jq -r '.result.id')
forge jobs wait "$JOB_ID"
```

**Output**

`result` is the queued `package.action` job, as shown by
[`forge jobs get`](/docs/reference/cli/jobs/#forge-jobs-get):

```json
{
  "schema": "forge.cli/v1",
  "result": { "id": "<job-id>", "kind": "package.action", "status": "pending", "…": "…" }
}
```

Shape from the server's schema; not captured on a live system.

Exit codes: `0` the job was queued; `1` you did not confirm; `2` an action
the package does not support, `upgrade`, an invalid id, or no `--yes` where
no prompt is possible; `3` not signed in; `4` permission denied; `5` MFA
needed; `6` no such installation; `7` the installation is busy with another
action; `9` server unreachable.

**See also**: [`forge packages installed`](#forge-packages-installed),
[`forge jobs wait`](/docs/reference/cli/jobs/#forge-jobs-wait).

### forge packages releases

Lists vendor release lines for the catalog's products: latest release, LTS,
and end of life. The data comes from [endoflife.date](https://endoflife.date)
and is refreshed daily. Needs only a signed-in session.

**Synopsis**

```text
forge packages releases [OPTIONS]
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
# Every release line
forge packages releases
# Supported PostgreSQL lines
forge -o json packages releases | jq -r '.result[] | select(.product == "postgresql" and .eol == "false") | .line'
```

**Output**

```text
$ forge packages releases
PRODUCT             LINE          LATEST                              LTS    EOL
apache-http-server  1.3           1.3.42                              false  2010-02-03
apache-http-server  2.0           2.0.65                              false  2013-07-10
apache-http-server  2.2           2.2.34                              false  2017-07-11
apache-http-server  2.4           2.4.70                              false  false
caddy               1             1.0.5                               false  2020-07-01
caddy               2             2.11.9                              false  false
…
```

```json
$ forge -o json packages releases
{
  "result": [
    {
      "eol": "2010-02-03",
      "fetched_at": 1794554316334,
      "latest": "1.3.42",
      "latest_at": "2010-02-02",
      "line": "1.3",
      "lts": false,
      "product": "apache-http-server",
      "released_at": "1998-06-01",
      "support_until": null
    },
    …
  ],
  "schema": "forge.cli/v1"
}
```

`eol` is a date, or `"false"` while the line is supported.

Exit codes: `0` success; `3` not signed in; `9` server unreachable.

**See also**: [`forge packages updates`](#forge-packages-updates).
