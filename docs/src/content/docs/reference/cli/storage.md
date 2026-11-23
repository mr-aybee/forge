---
title: forge storage
description: Every forge storage command for S3-compatible object stores, their buckets, objects, access keys and bucket grants, with flags, permissions, output and exit codes.
sidebar:
  order: 20
---

`forge storage` installs, connects and runs S3-compatible object stores, and
manages their buckets, objects, access keys and bucket grants. For how
stores, members, credentials and grants work, see
[Object storage](/docs/concepts/object-storage/). Every command takes the
[global flags](/docs/reference/cli/#global-flags).

`<store>` is a store id, name or slug. Names and slugs match without regard to
case. A name that matches more than one store fails with exit code `2` and
lists the matching ids.

Permissions are checked on the store's member node for a managed store, and on
the workspace for a connected store. For a store that belongs to an
environment, `view` and `storage.data` also count when you hold them on that
environment (environment). See [Ownership](/docs/concepts/object-storage/#ownership).
`storage.manage` needs an MFA-verified
session. On a terminal the CLI asks for your authenticator code. With
`--non-interactive` it fails with exit code `5`; run `forge auth step-up`
first. A `FORGE_TOKEN` cannot be stepped up; sign in with a context instead.

| Cedar action | Roles | MFA |
| --- | --- | --- |
| `view` | Every role on the node or workspace | no |
| `storage.data` | **Developer**, **Operator**, **Admin**, **Platform admin** | no |
| `storage.manage` | **Operator**, **Admin**, **Platform admin** | yes |

Set a store's environment at creation with `environment_id` in the spec of
[`forge storage create`](#forge-storage-create) or
[`forge storage connect`](#forge-storage-connect), or change it later with
[`forge storage owner`](#forge-storage-owner).

When you reach a store only through its environment, `forge storage list`
and `forge storage get` print the reduced store (`"limited": true`: no
credential, ports, data paths or jobs), and `forge storage keys` lists only
the keys attached to that environment's services.

A [bucket grant](/docs/concepts/object-storage/#bucket-grants) gives one account
access to the objects of one bucket without any of these roles. It issues no
S3 key. With only a grant you can run these commands:

| Command | What a grant holder gets |
| --- | --- |
| [`forge storage my-grants`](#forge-storage-my-grants) | The grants you hold |
| [`forge storage list`](#forge-storage-list), [`forge storage get`](#forge-storage-get) | The reduced store with `"by_grant": true`: an empty `endpoint`, no `members` and `environment_id` `null` |
| [`forge storage buckets`](#forge-storage-buckets) | Only the buckets you hold a live grant for |
| [`forge storage objects`](#forge-storage-objects) | The objects of a granted bucket |
| [`forge storage object-download`](#forge-storage-object-download) | One object of a granted bucket (`read` or `readwrite` grant) |
| [`forge storage object-upload`](#forge-storage-object-upload), [`forge storage object-delete`](#forge-storage-object-delete) | Upload or delete one object of a granted bucket (`readwrite` grant) |

Every other `forge storage` command still needs a role, including
[`forge storage grants`](#forge-storage-grants).

:::note
The outputs on this page come from the server's schema. The test system had
no object store, so only `forge storage list` was captured on a live system.
:::

## Commands

| Command | What it does |
| --- | --- |
| [`forge storage list`](#forge-storage-list) | Lists the object stores you can see |
| [`forge storage get`](#forge-storage-get) | Shows one store with its members |
| [`forge storage create`](#forge-storage-create) | Installs a managed RustFS store on nodes from a JSON spec |
| [`forge storage connect`](#forge-storage-connect) | Connects an external S3 endpoint from a JSON spec |
| [`forge storage adopt`](#forge-storage-adopt) | Adopts an App Store RustFS container as a managed store |
| [`forge storage action`](#forge-storage-action) | Runs a lifecycle action on a store |
| [`forge storage owner`](#forge-storage-owner) | Gives a store to an environment, or makes it shared again |
| [`forge storage test`](#forge-storage-test) | Checks that Forge reaches the store with its credential |
| [`forge storage object-upload`](#forge-storage-object-upload) | Uploads a local file as one object |
| [`forge storage object-download`](#forge-storage-object-download) | Downloads one object to a file or to stdout |
| [`forge storage object-delete`](#forge-storage-object-delete) | Deletes one object |
| [`forge storage buckets`](#forge-storage-buckets) | Lists the buckets of a store |
| [`forge storage bucket-create`](#forge-storage-bucket-create) | Creates a bucket |
| [`forge storage bucket-delete`](#forge-storage-bucket-delete) | Deletes an empty bucket |
| [`forge storage objects`](#forge-storage-objects) | Lists the objects in a bucket |
| [`forge storage keys`](#forge-storage-keys) | Lists the access keys of a store |
| [`forge storage key-create`](#forge-storage-key-create) | Issues an access key limited to one bucket |
| [`forge storage key-rotate`](#forge-storage-key-rotate) | Replaces an access key's secret |
| [`forge storage key-delete`](#forge-storage-key-delete) | Revokes an access key |
| [`forge storage grants`](#forge-storage-grants) | Lists the bucket grants of a store |
| [`forge storage grant-create`](#forge-storage-grant-create) | Grants one account access to one bucket, through Forge |
| [`forge storage grant-revoke`](#forge-storage-grant-revoke) | Revokes a bucket grant |
| [`forge storage my-grants`](#forge-storage-my-grants) | Lists the bucket grants you hold |

### forge storage list

Lists the object stores you can see. Needs `view` on each store, or a live
[bucket grant](#forge-storage-my-grants) on it; stores you cannot see are
left out.

**Synopsis**

```text
forge storage list [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

**Examples**

```sh
# List the stores you can see
forge storage list

# Print every field of every store as JSON
forge -o json storage list
```

**Output**

The test system had no store, so the captured list is empty:

```text
$ forge storage list
ID  NAME  PROVIDER  MODE  STATUS  ENDPOINT
```

```json
$ forge -o json storage list
{
  "result": [],
  "schema": "forge.cli/v1"
}
```

Each element of `result` has the fields shown under
[`forge storage get`](#forge-storage-get).

Exit codes:

- `0`: success, also when the list is empty.
- `3`: not signed in.
- `9`: the server cannot be reached.

**See also**: [`forge storage get`](#forge-storage-get),
[Object storage](/docs/concepts/object-storage/).

### forge storage get

Shows one store with its members. Needs `view` on the store, or a live
[bucket grant](#forge-storage-my-grants) on it. Through a grant alone you get
the reduced store: `id`, `name`, `slug`, `provider`, `mode`, `topology`,
`status`, `"limited": true`, `"by_grant": true`, an empty `endpoint`, no
`members` and `environment_id` `null`.

**Synopsis**

```text
forge storage get [OPTIONS] <STORE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<store>` | store id, name or slug | — | yes | The store to show | `media` |

**Examples**

```sh
# Show a store by name
forge storage get media

# Print only the status with jq
forge -o json storage get media | jq -r .result.status
```

**Output**

Table output prints the store as a key/value list. With `-o json` it is wrapped in
`{"result": …, "schema": "forge.cli/v1"}`. Shape from the server's schema; not
captured on a live system.

```json
{
  "id": "01900000-0000-7000-8000-000000000050",
  "name": "media",
  "slug": "media",
  "environment_id": null,
  "provider": "rustfs",
  "mode": "managed",
  "region": "us-east-1",
  "path_style": true,
  "version": "latest",
  "deployed_version": "<release>",
  "runtime": "native",
  "topology": "standalone",
  "s3_port": 9000,
  "console_port": null,
  "endpoint": "http://node-01:9000",
  "credential_id": "01900000-0000-7000-8000-000000000051",
  "status": "running",
  "last_error": null,
  "last_job_id": "01900000-0000-7000-8000-000000000052",
  "created_at": 1794590000000,
  "updated_at": 1794590100000,
  "members": [
    {
      "server_id": "01900000-0000-7000-8000-000000000004",
      "hostname": "node-01",
      "data_paths": ["/srv/rustfs"],
      "endpoint_host": "node-01",
      "require_mount": false,
      "status": "running",
      "capacity_bytes": 107374182400,
      "free_bytes": 96636764160,
      "dedicated": true,
      "observed_at": 1794590100000
    }
  ]
}
```

`environment_id` is the environment that owns the store, or `null` for a shared
store. `mode` is `managed` or `external`. A connected store has `runtime` and
`topology` `external` and no members. `credential_id` is the vault item that
holds the store's credential; the secret itself is never in the response.
Timestamps are epoch milliseconds.

Exit codes:

- `0`: success.
- `2`: the name matches more than one store.
- `4`: you lack `view` on the store and hold no live grant on it.
- `6`: no store with that id, name or slug that you can see.

**See also**: [`forge storage list`](#forge-storage-list),
[`forge storage test`](#forge-storage-test).

### forge storage create

Installs a managed RustFS store on one node (standalone) or several nodes
(distributed) from a JSON spec, and starts one install job per node. Needs
`storage.manage` on every member node (MFA).

**Synopsis**

```text
forge storage create [OPTIONS] --file <FILE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--file <FILE>` | path | — | yes | The JSON spec. `@<path>` also works, and `@-` reads the spec from stdin. The CLI checks the spec against the server's schema before it sends it | `--file store.json` |

The spec has these fields:

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `name` | string | — (required) | Display name, 1 to 80 characters |
| `slug` | string | — (required) | Instance name on the node: service, container and paths. Unique per node |
| `provider` | string | `rustfs` | The engine. RustFS is the only engine Forge installs |
| `version` | string | `latest` | The RustFS release |
| `runtime` | enum: `native`, `docker` | `native` | Windows nodes run `native` only |
| `topology` | enum: `standalone`, `distributed` | `standalone` | A standalone store has one member. A distributed store has 2 to 32 Linux members, the same number of drives on each, and at least 4 drives in total |
| `members` | list of objects | `[]` | One entry per node: `server_id` (node id), `data_paths` (one absolute path per drive) and optional `endpoint_host` (default: the node's hostname) |
| `server_id` | node id | — | Standalone shorthand, used with `data_path` when `members` is empty |
| `data_path` | string | — | Standalone shorthand: the data path on `server_id` |
| `endpoint_host` | string | node hostname | Standalone shorthand: the host the control plane reaches the node on |
| `require_mount` | boolean | `false` | Fail the install when a data path is not a mounted volume |
| `s3_port` | integer | `9000` | S3 API port. Unique per node |
| `console_port` | integer | none | RustFS console port. Unique per node |
| `environment_id` | environment id | none | The environment the store belongs to. Needs `deploy` on that environment as well. Omit it for a shared store |

Example spec for a standalone store:

```json
{
  "name": "media",
  "slug": "media",
  "runtime": "native",
  "topology": "standalone",
  "members": [
    { "server_id": "<node-id>", "data_paths": ["/srv/rustfs"] }
  ],
  "s3_port": 9000
}
```

Forge generates the store's root credential and seals it in the vault. You
never pass a secret in the spec.

**Examples**

```sh
# Install a store from a spec file
forge storage create --file store.json

# Read the spec from stdin
forge storage create --file @- < store.json

# Install, then print the id of the first install job
forge -o json storage create --file store.json | jq -r .result.job.id
```

**Output**

The server answers `202 Accepted` and the CLI returns at once; follow the jobs
with `forge jobs`. Shape from the server's
schema; not captured on a live system.

```json
{
  "store": { "id": "<store-id>", "name": "media", "status": "installing", "members": [ … ], … },
  "job": { "id": "<job-id>", "kind": "storage.action", "status": "queued", … },
  "jobs": [ { "id": "<job-id>", … } ]
}
```

`store` has the fields shown under [`forge storage get`](#forge-storage-get).
`job` is the first of `jobs`.

Exit codes:

- `0`: the install jobs are queued.
- `2`: the file is missing or not valid JSON, or the spec is invalid (for
  example a relative data path, or a Windows node in a distributed store).
- `4`: you lack `storage.manage` on a member node, or `deploy` on the
  environment in `environment_id`.
- `5`: an MFA-verified session is needed.
- `6`: a node is not in this workspace, or `environment_id` names no environment
  in it.
- `7`: a data path, slug or port on a node is already used by another store.

**See also**: [`forge storage action`](#forge-storage-action),
[Install an object store](/docs/guides/install-object-store/).

### forge storage connect

Connects an existing S3 service (AWS S3, R2, MinIO and others) from a JSON
spec. Forge tests the credential first and stores the secret in the vault.
Needs `storage.manage` on the workspace (MFA).

**Synopsis**

```text
forge storage connect [OPTIONS] --file <FILE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--file <FILE>` | path | — | yes | The JSON spec. `@<path>` also works, and `@-` reads the spec from stdin. The CLI checks the spec against the server's schema before it sends it | `--file external.json` |

The spec has these fields:

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `name` | string | — (required) | Display name, 1 to 80 characters |
| `provider` | enum: `aws`, `s3`, `rustfs`, `minio`, `garage`, `ceph`, `r2`, `wasabi`, `b2`, `other` | `s3` | A label for the service |
| `endpoint` | string | — (required) | `http(s)://host[:port]`, without credentials or a query. Loopback, link-local and metadata addresses are refused |
| `region` | string | `us-east-1` | Letters, digits and `-` |
| `path_style` | boolean | `false` | `endpoint/bucket/key` addressing instead of `bucket.endpoint/key`. Most self-hosted stores need `true` |
| `access_key` | string | — (required) | The access key id, up to 256 characters |
| `secret_key` | string | — (required) | The secret key, up to 512 characters |
| `bucket` | string | none | For a credential limited to one bucket: Forge checks it with this bucket instead of listing all buckets |
| `environment_id` | environment id | none | The environment the store belongs to. Needs `deploy` on that environment as well. Omit it for a shared store |

:::caution
The spec file holds the secret key. Create it from an environment variable,
pass it on stdin, and delete it afterwards. Never commit it.
:::

Example, built from environment variables and sent on stdin so the secret
never lands in a file:

```sh
jq -n --arg ak "$S3_ACCESS_KEY" --arg sk "$S3_SECRET_KEY" '{
  name: "backups",
  provider: "r2",
  endpoint: "https://<account-id>.r2.cloudflarestorage.com",
  region: "auto",
  path_style: true,
  access_key: $ak,
  secret_key: $sk,
  bucket: "<bucket>"
}' | forge storage connect --file @-
```

**Examples**

```sh
# Connect a store from a spec file
forge storage connect --file external.json

# Connect from a spec built on stdin (see above)
jq -n --arg ak "$S3_ACCESS_KEY" --arg sk "$S3_SECRET_KEY" \
  '{name: "archive", endpoint: "https://s3.<region>.amazonaws.com", region: "<region>", access_key: $ak, secret_key: $sk}' \
  | forge storage connect --file @-
```

**Output**

The server answers `201 Created` with the new store: the fields shown under
[`forge storage get`](#forge-storage-get), with `mode` `external`, `status`
`connected` and `members` `[]`. Shape from the server's schema; not captured
on a live system.

Exit codes:

- `0`: the store is connected.
- `2`: the file is missing or not valid JSON, or a field is invalid (endpoint,
  provider, region, empty keys).
- `4`: you lack `storage.manage` on the workspace, or `deploy` on the
  environment in `environment_id`.
- `5`: an MFA-verified session is needed.
- `6`: the `bucket` in the spec does not exist, or `environment_id` names no
  environment in this workspace.
- `9`: the service refused the credential or cannot be reached
  (`store_error` or `store_unreachable`).

**See also**: [`forge storage test`](#forge-storage-test),
[Connect S3 storage](/docs/guides/connect-s3-storage/).

### forge storage adopt

Takes over the App Store `rustfs` container on a node as a managed store,
without recreating it. The container, its data and its credential stay; the
credential moves into the vault and the App Store stops managing it. Needs
`storage.manage` on the node (MFA). The command creates a shared store; to
assign it to an environment afterwards, use
[`forge storage owner`](#forge-storage-owner).

**Synopsis**

```text
forge storage adopt [OPTIONS] <NAME> <NODE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<name>` | string | — | yes | Display name of the store, 1 to 80 characters | `media` |
| `<node>` | node id or name | — | yes | The node that runs the App Store RustFS container | `node-01` |
| `--endpoint-host <ENDPOINT_HOST>` | string | the node's hostname | no | Host or IP address the control plane reaches the node on | `--endpoint-host 10.0.0.5` |

**Examples**

```sh
# Adopt the App Store RustFS container on node-01
forge storage adopt media node-01

# Adopt it and reach it on a specific address
forge storage adopt media node-01 --endpoint-host <node-address>
```

**Output**

The server answers `202 Accepted` and the CLI returns at once. Shape from the
server's schema; not captured on a live system.

```json
{
  "store": { "id": "<store-id>", "name": "media", "slug": "rustfs", "runtime": "docker", "status": "adopting", … },
  "job": { "id": "<job-id>", "kind": "storage.action", "status": "queued", … }
}
```

:::caution
Adoption cannot be undone, and an adopted store cannot be reinstalled or
repaired. See [Adopt an App Store RustFS container](/docs/guides/adopt-app-store-rustfs/).
:::

Exit codes:

- `0`: the adopt job is queued.
- `2`: the endpoint host is not a hostname or IP address.
- `4`: you lack `storage.manage` on the node.
- `5`: an MFA-verified session is needed.
- `6`: the node is not found, or has no App Store RustFS installation.
- `7`: the container is already adopted, or another store on the node uses the
  name `rustfs` or the same port.

**See also**: [`forge storage get`](#forge-storage-get),
[Adopt an App Store RustFS container](/docs/guides/adopt-app-store-rustfs/).

### forge storage action

Runs a lifecycle action on a store, as one job per member node. Asks for
confirmation unless you pass `--yes`. Needs `storage.manage` on the store
(MFA); `status` needs `server.manage` instead and no MFA.

**Synopsis**

```text
forge storage action [OPTIONS] <STORE> <ACTION>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<store>` | store id, name or slug | — | yes | The store | `media` |
| `<action>` | enum | — | yes | For a managed store: `install` (repair), `start`, `stop`, `restart`, `status` (refresh), `uninstall`. For a connected store: `disconnect` only | `restart` |
| `--delete-data` | flag | off | no | With `uninstall` only: also delete the store's data paths on every member node. Any other action with this flag exits with `2` | `--delete-data` |
| `--yes` | flag | off | no | Skip the confirmation. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

| Action | What it does |
| --- | --- |
| `install` | Runs the install again for the store's release. Keeps data and the root credential. Refused for an adopted store |
| `start`, `stop`, `restart` | Starts, stops or restarts the service on every member |
| `status` | Refreshes the status and disk figures of every member |
| `uninstall` | Removes the service and firewall rules. Keeps the data paths unless you pass `--delete-data`; the root credential stays in the vault |
| `disconnect` | Removes a connected store and its vault credential. Takes effect at once, without a job |

:::caution
`uninstall` and `stop` make the store unavailable to every service that
uses it.
:::

:::caution
`uninstall --delete-data` deletes every object of the store on every member
node. It cannot be undone. It needs `storage.manage` and a step-up: the
server asks for an MFA-verified session for this request. The prompt
says what is deleted in capitals:

```text
Uninstall object store media AND DELETE ITS DATA on every node (cannot be undone)?
Type `yes` to continue:
```

The server also wants the store's name typed as a confirmation; the CLI
sends it for you after you answer `yes` or pass `--yes`.
:::

**Examples**

```sh
# Restart a store (asks for confirmation)
forge storage action media restart

# Refresh the status in a script
forge storage action media status --yes

# Disconnect a connected store
forge storage action backups disconnect --yes

# Remove a store and keep its data on the nodes
forge storage action media uninstall --yes

# Remove a store and delete its data on every node
forge storage action media uninstall --delete-data
```

**Output**

The server answers `202 Accepted` with the queued jobs, or `200` with
`{"disconnected": "<store-id>"}` for `disconnect`. Shape from the server's
schema; not captured on a live system.

```json
{
  "jobs": [ { "id": "<job-id>", "kind": "storage.action", "status": "queued", … } ]
}
```

Exit codes:

- `0`: the jobs are queued, or the store is disconnected.
- `1`: you answered something other than `yes` at the prompt.
- `2`: unknown action, any action other than `disconnect` on a connected
  store, `--delete-data` with an action other than `uninstall`, or no `--yes`
  in non-interactive use.
- `4`: you lack the needed permission.
- `5`: an MFA-verified session is needed.
- `6`: no store with that id, name or slug.
- `7`: another action is still running on the store, or `install` on an
  adopted store.

**See also**: [`forge storage get`](#forge-storage-get),
[Install an object store](/docs/guides/install-object-store/).

### forge storage owner

Gives a store to an environment, or makes it a shared store again. The roles
of the owning environment reach every bucket of the store: its members can
browse the store and attach its buckets to their services without a role on
the node. See [Ownership](/docs/concepts/object-storage/#ownership). Asks for
confirmation unless you pass `--yes`. Needs `storage.manage` on the store
(MFA) and `deploy` on the environment you name. Audited as
`storage.owner.set`.

**Synopsis**

```text
forge storage owner [OPTIONS] <--environment <NAMESPACE>|--clear> <STORE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<store>` | store id, name or slug | — | yes | The store | `media` |
| `--environment <NAMESPACE>` | environment id or name | — | one of `--environment` and `--clear` | The environment the store is to belong to. Cannot be combined with `--clear` | `--environment shop-prod` |
| `--clear` | flag | off | one of `--environment` and `--clear` | Make it a shared store: no environment owns it | `--clear` |
| `--force` | flag | off | no | Assign the store although buckets of it are attached to services of other environments. Cannot be combined with `--clear` | `--force` |
| `--yes` | flag | off | no | Skip the confirmation. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

:::caution
After the change, every role of the new environment that carries `view` or
`storage.data` reaches every bucket of the store. When buckets are attached
to services of other environments, the server refuses with exit `7`
(`attached_elsewhere`) and names those services. `--force` assigns the store
anyway; the audit event then lists the affected services.
:::

**Examples**

```sh
# Give the store to an environment (asks for confirmation)
forge storage owner media --environment shop-prod

# The same in a script
forge storage owner media --environment shop-prod --yes

# Assign it although other environments' services use its buckets
forge storage owner media --environment shop-prod --force --yes

# Make it a shared store again
forge storage owner media --clear --yes
```

**Output**

The store, as [`forge storage get`](#forge-storage-get) prints it:
a key/value list, or the store object under `result` with `-o json`. Shape
from the server's schema; not captured on a live system.

```json
{
  "result": {
    "id": "<store-id>",
    "name": "media",
    "slug": "media",
    "environment_id": "<environment-id>",
    "mode": "managed",
    "status": "running",
    "members": [ … ],
    …
  },
  "schema": "forge.cli/v1"
}
```

`environment_id` is the new owner, or `null` after `--clear`.

Exit codes:

- `0`: the owner is changed.
- `1`: you answered something other than `yes` at the prompt.
- `2`: neither or both of `--environment` and `--clear`, `--force` with
  `--clear`, a name that matches more than one store or environment, or no
  `--yes` in non-interactive use.
- `4`: you lack `storage.manage` on the store or `deploy` on the environment.
- `5`: an MFA-verified session is needed.
- `6`: no store or environment with that id or name that you can see.
- `7`: buckets of the store are attached to services of other environments
  (`attached_elsewhere`); pass `--force` to assign it anyway.

**See also**: [`forge storage get`](#forge-storage-get),
[`forge services storage attach`](/docs/reference/cli/services/#forge-services-storage-attach),
[Attach a bucket to a service](/docs/guides/attach-bucket-to-service/).

### forge storage test

Checks that Forge reaches the store with its stored credential, by listing the
buckets. Needs `storage.data` on the store.

**Synopsis**

```text
forge storage test [OPTIONS] <STORE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<store>` | store id, name or slug | — | yes | The store to test | `media` |

**Examples**

```sh
# Test a store
forge storage test media

# Stop a script when Forge cannot reach the store
forge storage test media || exit 1
```

**Output**

Shape from the server's schema; not captured on a live system.

```json
{ "ok": true, "latency_ms": 42, "buckets": 3 }
```

`latency_ms` is the time the bucket listing took, in milliseconds. `buckets`
is the number of buckets.

When the store refuses the request or cannot be reached, the CLI prints an
error with the code `store_unreachable` and the reason the store gave, and
exits with `9`:

```text
error: Forge cannot reach object store media: <reason> [store_unreachable]
```

Exit codes:

- `0`: Forge reaches the store with its credential.
- `4`: you lack `storage.data` on the store.
- `6`: no store with that id, name or slug.
- `7`: the store's credential is missing from the vault.
- `9`: the store refused the request or cannot be reached
  (`store_unreachable`), or the server cannot be reached.

**See also**: [`forge storage buckets`](#forge-storage-buckets),
[Connect S3 storage](/docs/guides/connect-s3-storage/).

### forge storage buckets

Lists the buckets of a store. Needs `storage.data` on the store, or a live
[bucket grant](#forge-storage-my-grants) on it. Through grants alone the
list holds only the buckets you hold a live grant for.

**Synopsis**

```text
forge storage buckets [OPTIONS] <STORE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<store>` | store id, name or slug | — | yes | The store | `media` |

**Examples**

```sh
# List the buckets of a store
forge storage buckets media

# Print only the bucket names
forge -o json storage buckets media | jq -r '.result[].name'
```

**Output**

Shape from the server's schema; not captured on a live system.

```text
NAME     CREATED
uploads  2026-01-01T00:00:00.000Z
backups  2026-01-02T00:00:00.000Z
```

```json
{
  "result": [
    { "name": "uploads", "created": "2026-01-01T00:00:00.000Z" },
    { "name": "backups", "created": "2026-01-02T00:00:00.000Z" }
  ],
  "schema": "forge.cli/v1"
}
```

`created` is the creation date the store reports, as text.

Exit codes:

- `0`: success.
- `9`: the store refused the request or cannot be reached.
- `4`: you lack `storage.data` on the store and hold no live grant on it.
- `6`: no store with that id, name or slug.

**See also**: [`forge storage objects`](#forge-storage-objects),
[Browse buckets and objects](/docs/guides/browse-objects/).

### forge storage bucket-create

Creates a bucket. Needs `storage.data` on the store.

**Synopsis**

```text
forge storage bucket-create [OPTIONS] <STORE> <BUCKET>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<store>` | store id, name or slug | — | yes | The store | `media` |
| `<bucket>` | string | — | yes | Bucket name: 3 to 63 lowercase letters, digits, dots and hyphens, starting and ending with a letter or digit | `uploads` |

**Examples**

```sh
# Create a bucket
forge storage bucket-create media uploads
```

**Output**

Shape from the server's schema; not captured on a live system.

```json
{ "name": "uploads" }
```

Exit codes:

- `0`: the bucket is created.
- `2`: the bucket name is invalid.
- `4`: you lack `storage.data` on the store.
- `6`: no store with that id, name or slug.
- `7`: the bucket already exists.

**See also**: [`forge storage bucket-delete`](#forge-storage-bucket-delete),
[`forge storage key-create`](#forge-storage-key-create).

### forge storage bucket-delete

Deletes an empty bucket. Asks for confirmation unless you pass `--yes`. Needs
`storage.manage` on the store (MFA).

**Synopsis**

```text
forge storage bucket-delete [OPTIONS] <STORE> <BUCKET>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<store>` | store id, name or slug | — | yes | The store | `media` |
| `<bucket>` | string | — | yes | The bucket to delete. It must be empty | `uploads` |
| `--yes` | flag | off | no | Skip the confirmation. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

**Examples**

```sh
# Delete a bucket (asks for confirmation)
forge storage bucket-delete media uploads

# Delete a bucket in a script
forge storage bucket-delete media uploads --yes
```

**Output**

On success the CLI prints nothing; with `-o json` it prints
`{"result": null, "schema": "forge.cli/v1"}`.

Exit codes:

- `0`: the bucket is deleted.
- `1`: you answered something other than `yes` at the prompt.
- `2`: no `--yes` in non-interactive use.
- `4`: you lack `storage.manage` on the store.
- `5`: an MFA-verified session is needed.
- `6`: no such store or bucket.
- `7`: the bucket is not empty.

**See also**: [`forge storage objects`](#forge-storage-objects),
[Browse buckets and objects](/docs/guides/browse-objects/).

### forge storage objects

Lists the objects in a bucket. Needs `storage.data` on the store, or a live
[bucket grant](#forge-storage-my-grants) for this bucket (`read` or
`readwrite`).

**Synopsis**

```text
forge storage objects [OPTIONS] <STORE> <BUCKET>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<store>` | store id, name or slug | — | yes | The store | `media` |
| `<bucket>` | string | — | yes | The bucket | `uploads` |
| `--prefix <PREFIX>` | string | none | no | List only keys that start with this prefix, up to 1024 bytes | `--prefix invoices/2026/` |
| `--token <TOKEN>` | string | none | no | Continue after a page: the `next_token` of the previous page. Cannot be combined with `--all` | `--token <next-token>` |
| `--limit <LIMIT>` | integer, 1 to 1000 | `200` | no | Objects per page | `--limit 1000` |
| `--all` | flag | off | no | Follow every page and list all objects | `--all` |

Without `--all` the command lists one page. With `--all` it requests page
after page until the store reports no more, so a large bucket takes many
requests; combine it with `--prefix` where you can.

**Examples**

```sh
# List the first page of a bucket
forge storage objects media uploads

# List one folder, every page
forge storage objects media uploads --prefix invoices/2026/ --all

# Continue after the first page, 1000 objects per page
forge storage objects media uploads --limit 1000 --token <next-token>

# Print the sub-folders of a folder
forge -o json storage objects media uploads --prefix invoices/ | jq -r '.result.prefixes[]'
```

**Output**

Shape from the server's schema; not captured on a live system. The table
shows the objects; sub-folders (`prefixes`) appear only in the JSON output.
When there are more objects, the table output also prints this line on
stderr:

```text
KEY                          SIZE    MODIFIED
invoices/2026/inv-0001.pdf   48213   2026-01-05T10:00:00.000Z
More objects: add --token <next-token> (or --all).
```

```json
{
  "result": {
    "objects": [
      {
        "key": "invoices/2026/inv-0001.pdf",
        "size": 48213,
        "last_modified": "2026-01-05T10:00:00.000Z",
        "etag": "\"<etag>\""
      }
    ],
    "prefixes": ["invoices/2026/archive/"],
    "next_token": null
  },
  "schema": "forge.cli/v1"
}
```

`size` is in bytes. `next_token` is set when there are more objects; pass it
to `--token` for the next page. With `--all`, `objects` and `prefixes` hold
every page and `next_token` is `null`.

Exit codes:

- `0`: success.
- `2`: the prefix or token is too long, or `--token` was combined with
  `--all`.
- `4`: you lack `storage.data` on the store and hold no live grant for the
  bucket.
- `6`: no such store or bucket.

**See also**: [`forge storage buckets`](#forge-storage-buckets),
[`forge storage object-download`](#forge-storage-object-download),
[Browse buckets and objects](/docs/guides/browse-objects/).

### forge storage object-upload

Uploads one local file as one object, through Forge. The CLI streams the
file from disk with its length; it never reads the whole file into memory.
An object with the same key is replaced. One object is at most 5 GiB.
`forge storage upload` is an alias. Needs `storage.data` on the store, or a
live [bucket grant](#forge-storage-my-grants) for this bucket with
`readwrite` access. Audited as `storage.object.upload`.

**Synopsis**

```text
forge storage object-upload [OPTIONS] --file <FILE> <STORE> <BUCKET> <KEY>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<store>` | store id, name or slug | — | yes | The store | `media` |
| `<bucket>` | string | — | yes | The bucket | `uploads` |
| `<key>` | string | — | yes | The object key. `/` separates folders | `backups/db.dump` |
| `--file <FILE>` | local path | — | yes | The file to upload | `--file ./db.dump` |
| `--content-type <CONTENT_TYPE>` | media type, up to 200 characters | `application/octet-stream` | no | The object's media type, stored with it | `--content-type application/pdf` |

**Examples**

```sh
# Upload a file
forge storage object-upload media uploads backups/db.dump --file ./db.dump

# Upload a PDF with its media type
forge storage upload media uploads invoices/inv-0001.pdf --file ./inv-0001.pdf --content-type service/pdf

# Print the uploaded size in bytes
forge -o json storage object-upload media uploads backups/db.dump --file ./db.dump | jq .result.size
```

**Output**

Shape from the server's schema; not captured on a live system.

```text
bucket  uploads
key     backups/db.dump
size    52428800
```

```json
{
  "result": {
    "bucket": "uploads",
    "key": "backups/db.dump",
    "size": 52428800
  },
  "schema": "forge.cli/v1"
}
```

`size` is in bytes. The CLI allows up to six hours for one transfer.

Exit codes:

- `0`: the object is stored.
- `1`: the file is over 5 GiB (`too_large`), or too many transfers run at
  once on the server (`busy`; try again shortly).
- `2`: the local file is missing or cannot be read, or a name that matches
  more than one store.
- `4`: you lack `storage.data` on the store and hold no live `readwrite`
  grant for the bucket.
- `6`: no such store or bucket.
- `9`: Forge cannot reach the store, or the server cannot be reached.

**See also**: [`forge storage objects`](#forge-storage-objects),
[`forge storage object-download`](#forge-storage-object-download),
[Browse buckets and objects](/docs/guides/browse-objects/).

### forge storage object-download

Downloads one object to a local file, or to stdout, through Forge. The body
is streamed to disk. An existing file is kept unless you pass `--force`, and
a download that breaks off leaves no partial file. `forge storage download`
is an alias. Needs `storage.data` on the store, or a live
[bucket grant](#forge-storage-my-grants) for this bucket (`read` or
`readwrite`).

:::note
Every download is recorded in the audit log as `storage.object.download`,
with the bucket, the key and the size. The CLI checks for an existing file
before it sends the request, so a refused download is not recorded as one.
:::

**Synopsis**

```text
forge storage object-download [OPTIONS] <STORE> <BUCKET> <KEY>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<store>` | store id, name or slug | — | yes | The store | `media` |
| `<bucket>` | string | — | yes | The bucket | `uploads` |
| `<key>` | string | — | yes | The object key | `backups/db.dump` |
| `--out <OUT>` | local path, or `-` | the last part of the key (after the last `/`), in the current folder | no | Where to write the object. `-` writes it to stdout and prints nothing else | `--out /tmp/db.dump` |
| `--force` | flag | off | no | Replace the file when it exists | `--force` |

Without `--out`, the key must end in a plain file name. A key that ends in
`/`, or whose last part is `.` or `..` or contains `\` or `:`, exits with `2`;
pass `--out <path>` for it.

**Examples**

```sh
# Download to ./db.dump
forge storage object-download media uploads backups/db.dump

# Download to another path, replacing the file there
forge storage download media uploads backups/db.dump --out /tmp/db.dump --force

# Pipe an object into another program
forge storage object-download media uploads backups/db.sql.gz --out - | gunzip | head
```

**Output**

Shape from the CLI's code; not captured on a live system.

```text
bucket  uploads
key     backups/db.dump
size    52428800
path    db.dump
```

```json
{
  "result": {
    "bucket": "uploads",
    "key": "backups/db.dump",
    "size": 52428800,
    "path": "db.dump"
  },
  "schema": "forge.cli/v1"
}
```

`size` is the number of bytes written. `path` is the file as you named it.
With `--out -`, stdout carries only the object's bytes, in every output
format.

Exit codes:

- `0`: the object is written.
- `1`: too many transfers run at once on the server (`busy`; try again
  shortly).
- `2`: the key gives no file name and `--out` is missing, the target folder
  does not exist or is not writable, or a name that matches more than one
  store.
- `4`: you lack `storage.data` on the store and hold no live grant for the
  bucket.
- `6`: no such store, bucket or object.
- `7`: the target file exists (`file_exists`); pass `--force` to replace it.
- `9`: the download broke off, Forge cannot reach the store, or the server
  cannot be reached.

**See also**: [`forge storage objects`](#forge-storage-objects),
[`forge storage object-upload`](#forge-storage-object-upload).

### forge storage object-delete

Deletes one object. Asks for confirmation unless you pass `--yes`. Needs
`storage.data` on the store, or a live
[bucket grant](#forge-storage-my-grants) for this bucket with `readwrite`
access. Audited as `storage.object.delete`.

**Synopsis**

```text
forge storage object-delete [OPTIONS] <STORE> <BUCKET> <KEY>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<store>` | store id, name or slug | — | yes | The store | `media` |
| `<bucket>` | string | — | yes | The bucket | `uploads` |
| `<key>` | string | — | yes | The object key | `backups/db.dump` |
| `--yes` | flag | off | no | Skip the confirmation. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

:::caution
A deleted object cannot be restored through Forge.
:::

**Examples**

```sh
# Delete an object (asks for confirmation)
forge storage object-delete media uploads backups/db.dump

# Delete it in a script
forge storage object-delete media uploads backups/db.dump --yes
```

**Output**

Shape from the CLI's code; not captured on a live system.

```text
deleted  backups/db.dump
bucket   uploads
```

```json
{
  "result": {
    "deleted": "backups/db.dump",
    "bucket": "uploads"
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: the object is deleted.
- `1`: you answered something other than `yes` at the prompt.
- `2`: a name that matches more than one store, or no `--yes` in
  non-interactive use.
- `4`: you lack `storage.data` on the store and hold no live `readwrite`
  grant for the bucket.
- `6`: no such store or bucket.
- `9`: Forge cannot reach the store, or the server cannot be reached.

**See also**: [`forge storage objects`](#forge-storage-objects),
[`forge storage bucket-delete`](#forge-storage-bucket-delete).

### forge storage keys

Lists the access keys of a managed RustFS store. Needs `storage.data` on the
store. Connected stores manage keys at their provider.

**Synopsis**

```text
forge storage keys [OPTIONS] <STORE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<store>` | store id, name or slug | — | yes | A managed RustFS store | `media` |

**Examples**

```sh
# List the access keys of a store
forge storage keys media

# List only the keys Forge issued
forge -o json storage keys media | jq '.result[] | select(.managed_by_forge)'
```

**Output**

Shape from the server's schema; not captured on a live system.

```text
ACCESS KEY      NAME         BUCKET   ACCESS
<access-key>    billing-api  uploads  read
```

```json
{
  "result": [
    {
      "access_key": "<access-key>",
      "name": "billing-api",
      "description": "Forge app key for bucket uploads",
      "status": "<status>",
      "expiration": null,
      "vault_item_id": "<vault-item-id>",
      "bucket": "uploads",
      "access": "read",
      "managed_by_forge": true
    }
  ],
  "schema": "forge.cli/v1"
}
```

Keys created another way, for example in the RustFS console, have
`managed_by_forge: false` and no `bucket`, `access` or `vault_item_id`.
`status` and `expiration` are what RustFS reports.

Exit codes:

- `0`: success.
- `4`: you lack `storage.data` on the store.
- `6`: no store with that id, name or slug.
- `7`: the store is not a managed RustFS store.

**See also**: [`forge storage key-create`](#forge-storage-key-create),
[Issue object store access keys](/docs/guides/object-store-access-keys/).

### forge storage key-create

Issues an access key for one service, limited to one existing bucket of a
managed RustFS store. Needs `storage.manage` on the store (MFA).

:::caution
The secret goes only into the credential vault, never to the terminal. The
response holds the access key and the vault item id. A platform administrator
reveals the secret with
[`forge vault reveal`](/docs/reference/cli/vault/#forge-vault-reveal).
:::

**Synopsis**

```text
forge storage key-create [OPTIONS] --bucket <BUCKET> <STORE> <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<store>` | store id, name or slug | — | yes | A managed RustFS store | `media` |
| `<name>` | string | — | yes | What the key is for, a service name: 1 to 32 letters, digits, `-`, `_` or `.` | `billing-api` |
| `--bucket <BUCKET>` | string | — | yes | The bucket the key may reach. It must exist | `--bucket uploads` |
| `--access <ACCESS>` | enum: `read`, `readwrite` | `readwrite` | no | Read-only or read-write access to the bucket | `--access read` |

**Examples**

```sh
# Issue a read-write key for one bucket
forge storage key-create media billing-api --bucket uploads

# Issue a read-only key
forge storage key-create media reports --bucket uploads --access read
```

**Output**

The server answers `201 Created`. Shape from the server's schema; not captured
on a live system. In table mode the CLI first prints where the secret is, on
stderr, then the key as a key/value list:

```text
The secret is in the vault: `forge vault reveal <vault-item-id>`.
{
  "access_key": "<access-key>",
  "name": "billing-api",
  "bucket": "uploads",
  "access": "readwrite",
  "vault_item_id": "<vault-item-id>"
}
```

With `-o json` only the object is printed, wrapped in
`{"result": …, "schema": "forge.cli/v1"}`.

The vault item is named `<store name> / <bucket> (<name>)`. Its username is
the access key.

Exit codes:

- `0`: the key is issued.
- `2`: the name, bucket name or access value is invalid.
- `4`: you lack `storage.manage` on the store.
- `5`: an MFA-verified session is needed.
- `6`: no such store or bucket.
- `7`: the store is not a managed RustFS store.

**See also**: [`forge storage keys`](#forge-storage-keys),
[`forge vault reveal`](/docs/reference/cli/vault/#forge-vault-reveal),
[Issue object store access keys](/docs/guides/object-store-access-keys/).

### forge storage key-rotate

Replaces a Forge-issued key. Forge creates a new key with the same name,
bucket and access, tests it on the bucket, puts the new secret in place of the
old one in the same vault item, then deletes the old key. The secret never
goes to the terminal. Asks for confirmation unless you pass `--yes`. Needs
`storage.manage` on the store (MFA).

:::caution
The old key is deleted as soon as the new one works, so services that use
it stop working at once. Give them the new access key and the new secret from
the vault ([`forge vault reveal`](/docs/reference/cli/vault/#forge-vault-reveal))
right after.
:::

**Synopsis**

```text
forge storage key-rotate [OPTIONS] <STORE> <ACCESS_KEY>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<store>` | store id, name or slug | — | yes | A managed RustFS store | `media` |
| `<access-key>` | string | — | yes | The access key to replace (`<access_key>` in the help). Only keys Forge issued can be rotated | `<access-key>` |
| `--yes` | flag | off | no | Skip the confirmation. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

**Examples**

```sh
# Rotate a key (asks for confirmation)
forge storage key-rotate media <access-key>

# Rotate a key in a script and print the new access key
forge -o json storage key-rotate media <access-key> --yes | jq -r .result.access_key
```

**Output**

Shape from the server's schema; not captured on a live system.

```json
{
  "access_key": "<new-access-key>",
  "bucket": "uploads",
  "access": "readwrite",
  "vault_item_id": "<vault-item-id>",
  "old_key_retired": true,
  "warning": null
}
```

When the old key could not be deleted, `old_key_retired` is `false` and
`warning` says so. The new key is in place; delete the old one with
[`forge storage key-delete`](#forge-storage-key-delete).

Exit codes:

- `0`: the key is rotated.
- `1`: you answered something other than `yes` at the prompt.
- `9`: the new key did not work (`rotation_failed`); the old key stays.
- `2`: no `--yes` in non-interactive use.
- `4`: you lack `storage.manage` on the store.
- `5`: an MFA-verified session is needed.
- `6`: no such store, or no Forge-issued key with that access key.
- `7`: the store is not a managed RustFS store.

**See also**: [`forge storage keys`](#forge-storage-keys),
[Issue object store access keys](/docs/guides/object-store-access-keys/).

### forge storage key-delete

Revokes an access key at the store and deletes its vault item. Asks for
confirmation unless you pass `--yes`. Needs `storage.manage` on the store
(MFA).

**Synopsis**

```text
forge storage key-delete [OPTIONS] <STORE> <ACCESS_KEY>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<store>` | store id, name or slug | — | yes | A managed RustFS store | `media` |
| `<access-key>` | string | — | yes | The access key to revoke (`<access_key>` in the help). The store's root key cannot be revoked here | `<access-key>` |
| `--yes` | flag | off | no | Skip the confirmation. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

**Examples**

```sh
# Revoke a key (asks for confirmation)
forge storage key-delete media <access-key>

# Revoke a key in a script
forge storage key-delete media <access-key> --yes
```

**Output**

On success the CLI prints nothing; with `-o json` it prints
`{"result": null, "schema": "forge.cli/v1"}`.

Exit codes:

- `0`: the key is revoked.
- `1`: you answered something other than `yes` at the prompt.
- `2`: no `--yes` in non-interactive use.
- `4`: you lack `storage.manage` on the store.
- `5`: an MFA-verified session is needed.
- `6`: no such store or key.
- `7`: the key is the store's root key, or the store is not a managed RustFS
  store.

**See also**: [`forge storage key-rotate`](#forge-storage-key-rotate),
[Issue object store access keys](/docs/guides/object-store-access-keys/).

### forge storage grants

Lists the bucket grants of a store: who may open which bucket through Forge
without a role, with which access and until when. Live grants by default,
newest first. Needs `storage.manage` on the store (MFA). Holding a grant does
not let you list grants; use
[`forge storage my-grants`](#forge-storage-my-grants) for your own.

**Synopsis**

```text
forge storage grants [OPTIONS] <STORE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<store>` | store id, name or slug | — | yes | The store | `media` |
| `--all` | flag | off | no | Include expired and revoked grants | `--all` |
| `--limit <LIMIT>` | integer, 1 to 500 | `200` | no | How many grants to list, newest first. The server brings a value outside the range to the nearest bound | `--limit 500` |

**Examples**

```sh
# List the live grants of a store
forge storage grants media

# Include grants that expired or were revoked
forge storage grants media --all

# Print who holds a grant for one bucket
forge -o json storage grants media | jq -r '.result[] | select(.bucket == "uploads") | .user'
```

**Output**

Shape from the server's schema; not captured on a live system.

```text
$ forge storage grants media --all
ID                                    USER             BUCKET   ACCESS     EXPIRES  STATUS   GRANTED BY  NOTE
01900000-0000-7000-8000-000000000061  ada              uploads  read       never    active   root        design review
01900000-0000-7000-8000-000000000062  dev@example.com  uploads  readwrite  in 3 d   active   root
01900000-0000-7000-8000-000000000063  ada              backups  read       -        revoked  root
```

`EXPIRES` is `never`, the time left (`in 8 h`, `in 3 d`), or `-` for a grant
that is expired or revoked. `GRANTED BY` is the sign-in name of the account
that issued the grant, or its principal (`user:<id>`) when the name is not
known.

With `-o json`, `result` is the list of grants:

```json
{
  "result": [
    {
      "id": "01900000-0000-7000-8000-000000000061",
      "store_id": "01900000-0000-7000-8000-000000000050",
      "store": "media",
      "user_id": "01900000-0000-7000-8000-000000000012",
      "user": "ada",
      "bucket": "uploads",
      "access": "read",
      "created_by": "user:01900000-0000-7000-8000-000000000003",
      "created_by_name": "root",
      "created_at": 1794590000000,
      "expires_at": null,
      "never_expires": true,
      "revoked_at": null,
      "note": "design review",
      "status": "active",
      "expires": "never",
      "granted_by": "root"
    }
  ],
  "schema": "forge.cli/v1"
}
```

`access` is `read` or `readwrite`. `status` is `active`, `expired` or
`revoked`. `expires_at` is `null` for a grant that never expires. Timestamps
are epoch milliseconds. `expires` and `granted_by` are the texts of the table
columns, added by the CLI; the other fields come from the
[REST API](/docs/reference/rest-api/#bucket-grants).

Exit codes:

- `0`: success, also when the list is empty.
- `2`: the name matches more than one store, or `--limit` is not a number.
- `4`: you lack `storage.manage` on the store.
- `5`: an MFA-verified session is needed.
- `6`: no store with that id, name or slug.

**See also**: [`forge storage grant-create`](#forge-storage-grant-create),
[`forge storage my-grants`](#forge-storage-my-grants),
[Grant a person access to a bucket](/docs/guides/grant-bucket-access/).

### forge storage grant-create

Grants one account access to one existing bucket of a store, without a role
on the store. The access is through Forge only: the console, the REST API
and this CLI. Needs `storage.manage` on the store (MFA). Audited as
`storage.grant.create`.

:::note
A grant does not issue an S3 key, and its holder gets no S3 endpoint or
credential. For an S3 client or a service, use
[`forge storage key-create`](#forge-storage-key-create).
:::

**Synopsis**

```text
forge storage grant-create [OPTIONS] --user <USER> --bucket <BUCKET> <STORE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<store>` | store id, name or slug | — | yes | The store, managed or connected | `media` |
| `--user <USER>` | username, email or user id | — | yes | The account that gets the access. It must exist and not be disabled. A service account works too | `--user ada` |
| `--bucket <BUCKET>` | string | — | yes | The bucket. It must exist; the name is matched exactly | `--bucket uploads` |
| `--access <ACCESS>` | enum: `read`, `readwrite` | `read` | no | `read`: list and download objects. `readwrite`: also upload and delete objects | `--access readwrite` |
| `--for <DURATION>` | `<n>m`, `<n>h`, `<n>d`, `<n>w` (combinable: `1d12h`) or `never` | `never` | no | How long the grant lasts, from 1 minute to 10 years. `never` means until someone revokes it | `--for 8h` |
| `--note <NOTE>` | string | none | no | Why the grant exists. Shown in the list and stored in the audit log. The server drops control characters and keeps the first 200 characters | `--note "design review"` |

An account holds at most one live grant per bucket. To change the access or
the expiry, revoke the grant with
[`forge storage grant-revoke`](#forge-storage-grant-revoke) and create a new
one.

**Examples**

```sh
# Read-only access until someone revokes it
forge storage grant-create media --user ada --bucket uploads

# Read and write for three days, with a reason
forge storage grant-create media --user dev@example.com --bucket uploads \
  --access readwrite --for 3d --note "import run"

# Create a grant in a script and print its id
forge -o json storage grant-create media --user ada --bucket uploads --for 2w | jq -r .result.id
```

**Output**

The server answers `201 Created`. Shape from the server's schema; not
captured on a live system. In table mode the CLI first prints a summary line
on stderr, then the grant as a key/value list:

```text
$ forge storage grant-create media --user ada --bucket uploads
ada may open bucket uploads of media (read) through Forge · never expires
{
  "id": "01900000-0000-7000-8000-000000000061",
  "store_id": "01900000-0000-7000-8000-000000000050",
  "store": "media",
  "user_id": "01900000-0000-7000-8000-000000000012",
  "user": "ada",
  "bucket": "uploads",
  "access": "read",
  "created_by": "user:01900000-0000-7000-8000-000000000003",
  "created_by_name": "root",
  "created_at": 1794590000000,
  "expires_at": null,
  "never_expires": true,
  "revoked_at": null,
  "note": "",
  "status": "active"
}
```

With `--for`, the summary line ends in the time left, for example
`· expires in 3 d`. With `-o json` only the object is printed, wrapped in
`{"result": …, "schema": "forge.cli/v1"}`.

Exit codes:

- `0`: the grant is created.
- `2`: `--user` or `--bucket` is missing, the `--access` or `--for` value is
  invalid, or the lifetime is longer than 10 years.
- `4`: you lack `storage.manage` on the store.
- `5`: an MFA-verified session is needed.
- `6`: no such store or bucket, or `unknown or disabled user`: the account
  does not exist or is disabled (one answer for both).
- `7`: the account already holds a live grant for this bucket.
- `9`: the store refused the bucket listing or cannot be reached, so Forge
  could not check the bucket.

**See also**: [`forge storage grants`](#forge-storage-grants),
[`forge storage grant-revoke`](#forge-storage-grant-revoke),
[Grant a person access to a bucket](/docs/guides/grant-bucket-access/).

### forge storage grant-revoke

Revokes a bucket grant. The holder's next request on the bucket is refused.
Asks for confirmation unless you pass `--yes`. Needs `storage.manage` on the
store (MFA). Audited as `storage.grant.revoke`.

**Synopsis**

```text
forge storage grant-revoke [OPTIONS] <STORE> <GRANT_ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<store>` | store id, name or slug | — | yes | The store the grant belongs to | `media` |
| `<grant-id>` | grant id | — | yes | The grant to revoke (`<grant_id>` in the help), from [`forge storage grants`](#forge-storage-grants) | `01900000-0000-7000-8000-000000000061` |
| `--yes` | flag | off | no | Skip the confirmation. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

**Examples**

```sh
# Revoke a grant (asks for confirmation)
forge storage grant-revoke media <grant-id>

# Revoke a grant in a script
forge storage grant-revoke media <grant-id> --yes
```

**Output**

Shape from the server's schema; not captured on a live system. Without
`--yes` the CLI asks first:

```text
Revoke bucket grant <grant-id> of media? Its holder loses the bucket at once
```

Then it prints the id of the revoked grant:

```json
{
  "revoked": "<grant-id>"
}
```

With `-o json` the object is wrapped in
`{"result": …, "schema": "forge.cli/v1"}`. Revoking a grant that is already
revoked succeeds and keeps the time of the first revocation.

Exit codes:

- `0`: the grant is revoked.
- `1`: you answered something other than `yes` at the prompt.
- `2`: no `--yes` in non-interactive use, or the grant id is not valid.
- `4`: you lack `storage.manage` on the store.
- `5`: an MFA-verified session is needed.
- `6`: no such store, or no grant with that id on this store.

**See also**: [`forge storage grants`](#forge-storage-grants),
[`forge storage grant-create`](#forge-storage-grant-create).

### forge storage my-grants

Lists the live bucket grants you hold: the buckets you may open through
Forge, with which access and until when. The list does not say who granted
the access or why. Every signed-in account may run it; it needs no
role. Expired and revoked grants are not listed. The command takes no
arguments and no flags of its own.

**Synopsis**

```text
forge storage my-grants [OPTIONS]
```

**Examples**

```sh
# List the buckets you may open
forge storage my-grants

# Browse a granted bucket
forge storage objects <store> <bucket>

# Print the granted buckets as store/bucket pairs
forge -o json storage my-grants | jq -r '.result[] | "\(.store)/\(.bucket)"'
```

**Output**

Shape from the server's schema; not captured on a live system.

```text
$ forge storage my-grants
STORE  BUCKET   ACCESS  EXPIRES  ID
media  uploads  read    never    01900000-0000-7000-8000-000000000061
media  reports  read    in 8 h   01900000-0000-7000-8000-000000000064
```

With `-o json`, each element of `result` has `id`, `store_id`, `store` (the
store's name), `bucket`, `access`, `expires_at` (epoch milliseconds, or
`null` for a grant that never expires), `status` and `expires` (the text of
the table column).

The list is empty when you hold no live grant, when your account is disabled,
and for an AI connector (MCP): grants do not count for connectors.

Exit codes:

- `0`: success, also when the list is empty.
- `3`: not signed in.
- `9`: the server cannot be reached.

**See also**: [`forge storage objects`](#forge-storage-objects),
[`forge storage buckets`](#forge-storage-buckets),
[Bucket grants](/docs/concepts/object-storage/#bucket-grants).
