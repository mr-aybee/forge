---
title: Object storage
description: How Forge installs, connects and talks to S3-compatible object stores, and where the limits are.
sidebar:
  order: 8
---

This page explains how Forge models object storage: stores Forge runs for you,
stores you connect, who owns a store, how one person gets access to one
bucket, where S3 calls run and how credentials and data are protected. To set one up, see [Install an object store](/docs/guides/install-object-store/),
[Adopt an App Store RustFS container](/docs/guides/adopt-app-store-rustfs/)
or [Connect S3 storage](/docs/guides/connect-s3-storage/).

## Managed and connected stores

An **object store** is one S3 deployment. It has one of two modes:

| Mode | Created with | Forge does | Examples |
| --- | --- | --- | --- |
| `managed` | **Install RustFS**, or adopting an App Store `rustfs` container | Installs RustFS on one or more nodes and runs its lifecycle (install, start, stop, restart, status, uninstall) through durable jobs. Browses buckets and objects, and issues bucket-scoped access keys | RustFS on your nodes |
| `external` | **Connect existing** | Browses buckets and objects only. No lifecycle, no access keys | AWS S3, Cloudflare R2, Wasabi, Backblaze B2, MinIO, Ceph, or any other S3-compatible service that Forge does not manage |

RustFS is the only engine Forge installs. Disconnecting an external store
removes it and its credential from Forge. It never touches the data at the
provider.

## Ownership

A store is either **shared** or belongs to one **environment** (an environment in
the API). A shared store has no owner (`environment_id: null`). Every store is
shared until you assign it, and a shared store behaves as described on the
rest of this page.

Ownership changes who may use the store, and nothing else. Three paths lead
to a store:

- A role on the store's first member node for a managed store, or on the
  workspace for a connected store. Forge always checks this first.
- A role on the environment that owns the store, for `view` and
  `storage.data` only. This section describes it.
- A [bucket grant](#bucket-grants), for the objects of one bucket only.

For a store with an owner, Forge accepts two actions when you hold them on
the owning environment:

| Task | Shared store | Store owned by an environment |
| --- | --- | --- |
| See the store in the list and open it | `view` on the node or workspace | The same, or `view` on the environment (a reduced view, see below) |
| Test the connection, list and create buckets, browse, upload, download and delete objects, list access keys | `storage.data` on the node or workspace | The same, or `storage.data` on the environment (only that environment's keys are listed) |
| Attach a bucket to a service | `deploy` on the service's environment and `storage.manage` (MFA) on the store | For a service of the owning environment: `deploy` on that environment, unless a service of another environment already uses the bucket. That bucket, and any other service: as for a shared store |
| Detach a bucket from a service | `deploy` on the service's environment | The same |
| Install, repair, start, stop, restart, enable, disable, uninstall, disconnect | `storage.manage` (MFA) on the node or workspace | The same |
| Delete a bucket; issue, rotate or revoke an access key | `storage.manage` (MFA) on the node or workspace | The same |
| Change or clear the owner | `storage.manage` (MFA) on the node or workspace, and `deploy` on the environment you assign it to | The same |

The built-in roles map onto this as follows. On the owning environment, a
**Developer**, **Operator** or **Admin** can browse the store and attach its
buckets to that environment's services. An **Auditor**, **DBA** or
**Customer** can see the store, but not its buckets. A role on another
environment gains nothing from the ownership. A role on a service
covers the service's environments, so it also covers their stores.
Roles on the store's member node, or on the workspace for a connected store,
work as they did before, whoever owns the store.

The boundary of ownership is the store, not the bucket. The roles of the
owning environment reach **every** bucket of the store: they can read,
change and delete its objects and attach its buckets to their services. Let
an environment own a store only when all the store's buckets belong to that
environment. Keep a store that several environments use shared, so that it
stays with the people who hold `storage.manage`.

Forge guards the two places where this matters:

- It refuses to assign a store to an environment while buckets of the store
  are attached to services of other environments. You can confirm the
  assignment anyway; the audit event then names those services and buckets.
- Inside the owning environment, a bucket that a service of another
  environment already uses cannot be attached with `deploy` alone. It needs
  `storage.manage` on the store, as on a shared store.

Detaching is different: it only takes access away from your own service. It
needs `deploy` on the service's environment, whoever owns the store.

A person who reaches a store only through its environment sees less of it:

- The store's name, provider, status, endpoint and environment, and the name
  and status of each member node. Not its credential, data paths, ports,
  capacity, last error or jobs.
- Of its access keys, only the keys attached to services of that
  environment. Standalone keys and the keys of other environments are left
  out.

Two trade-offs follow from this model:

- Attaching a bucket inside the owning environment issues an access key
  without `storage.manage`, so without an MFA step-up. The key is limited to
  one bucket, and the person could already browse that store.
- Everyone with `view` on the owning environment sees the store's endpoint
  and the names of its member nodes.

`storage.manage` is never decided on an environment. It runs scripts as root
on the node, so it stays with the people who hold it on the node or the
workspace. They keep full control of the store whoever owns it.

Changing or clearing the owner moves no data and changes no key. Buckets that
are already attached keep working, also for services of other environments.
Clearing the owner is always allowed.
An environment that owns a store cannot be deleted until you assign the store
elsewhere or clear its owner.

What ownership does not cover:

- A store has one owner. Several environments cannot own one store.
- A service cannot own a store directly. Assign the store to one of the
  service's environments.
- Access through ownership is per store, not per bucket. A person who may
  browse the store may browse every bucket in it. To open one bucket to one
  person, use a [bucket grant](#bucket-grants).

To assign a store, see
[Assign a store to an environment](/docs/guides/install-object-store/#assign-a-store-to-an-environment).

## Members and drives

A managed store has one or more **members**. A member is a node plus its
**drives**: data paths that belong to this store only, up to 16 per node.
Forge records each member's capacity and free bytes, and whether its data
paths are dedicated mounts.

When you turn on **require a dedicated mount** (`require_mount`), Forge
refuses to install or start the store unless every data path is its own mount
point. On Linux the systemd unit also gets `ExecStartPre=mountpoint -q`. This
stops RustFS from silently writing to the root disk when a volume failed to
mount at boot.

## Standalone and distributed

| | `standalone` | `distributed` (high availability) |
| --- | --- | --- |
| Nodes | 1 | 2 to 32, Linux only |
| Drives | 1 to 16 on the node (Windows: 1) | The same number on every node, 4 to 64 in total, at most 16 per node |
| Survives | Nothing beyond the node's own disk redundancy | Loss of drives or nodes within the erasure-code parity |
| Runtimes | `native`, `docker` (Linux) | `native`, `docker` |

A **distributed** store is one RustFS erasure-coded cluster. Every member gets
the same drive list in `RUSTFS_VOLUMES`: `http://<endpoint_host>:<s3_port><path>`
for every drive of every member, in member order, together with the same S3
port and the same credential. The members talk to each other over that S3
port, so each member must resolve and reach every other member on
`<endpoint_host>:<s3_port>`.

Forge starts one install job per member, in parallel. Each member waits up to
15 minutes for the others (`RUSTFS_STARTUP_READINESS_MAX_WAIT_SECS=900`)
before it answers its health check. With the `docker` runtime, distributed
members use `--network host` and mount each data path at the same path inside
the container, so the addresses in the drive list are the members' own.

### Failure domains and status

RustFS splits objects into data and parity shards across the drives. Losing
more drives or nodes than the parity allows stops writes, then reads. The
default parity depends on the number of drives per erasure set; see the
[RustFS documentation](https://docs.rustfs.com/) for the exact figures.

A failure domain is anything that takes several drives down at once. Forge
treats the **node** as the failure unit: several drives of one node may share a
disk, because the node fails as a whole anyway. Forge then warns in the job log
and sets `RUSTFS_UNSAFE_BYPASS_DISK_CHECK=true` for that member, which RustFS
otherwise requires for drives on one device. Drives that live on one NAS or one storage array are also
**one** failure domain, even when they are mounted on different nodes: if the
NAS fails, all of them fail together. Spread drives over independent disks and
nodes.

The store's status summarises its members:

| Status | Meaning |
| --- | --- |
| `running` | Every member runs |
| `degraded` | Some members run. The cluster keeps serving data within its parity |
| `disabled` | Stopped on purpose with **Disable**. Data and settings stay until **Enable** |
| `failed`, `error`, `missing`, `stopped` | No member runs, or a member reported this state |

A first install that fails on every member is rolled back. Forge uninstalls on
each node, keeping the data paths, and deletes the store record and its vault
credential. The failed jobs and their logs stay in **Jobs**, and the audit log
records `storage.install` with outcome `failure` and `rolled_back: true`.

### What Forge does not do yet

- Forge's own bucket and object calls fail over to the first member that
  accepts a connection. Your services need their own load balancer or DNS
  name across the members; Forge does not create one yet.
- A standalone store cannot become distributed. Create a new distributed store
  and copy the data across.
- Expanding a cluster with a new pool or more nodes is not supported yet.

## Data paths

Data paths must be absolute directories outside system locations, for example
`/srv/objstore/media` or `D:\objstore`. Forge rejects `/`, `/etc`, `/usr`,
`/tmp`, `/opt/forge`, `/var/lib/forge` and bare top-level directories such as
`/srv` or `/home`; on Windows it rejects `\Windows`, `\Program Files`,
`\ProgramData` and `\Users`. The one exception is an
[adopted](#adoption) App Store container, which keeps its data in
`/var/lib/forge/pkgs/rustfs`.

## Several stores per node

One node can run several RustFS instances. Each needs its own instance name
(the slug, `[a-z][a-z0-9-]{0,30}`), its own S3 and console ports (1024 or
higher) and its own data paths. Forge returns `409` when the name, a port or a
data path is already taken on that node.

| | Linux (`native`) | Linux (`docker`) | Windows (`native`) |
| --- | --- | --- | --- |
| Binary | Release zip from GitHub, verified against `SHA256SUMS`, in `/opt/forge/objstore/<slug>/<release>` | Image `rustfs/rustfs:<release>` | `rustfs.exe` in `%ProgramData%\OrbynForge\objstore\<slug>\<release>` |
| Runs as | systemd unit `forge-objstore-<slug>`, user `forge-objstore` (uid 10001) | Container `forge-objstore-<slug>`, data directory owned by uid 10001 | `SYSTEM` scheduled task `forge-objstore-<slug>` at startup, restarts on failure |
| Settings | `/etc/forge/objstore/<slug>.env` (mode 0600) | same | `%ProgramData%\OrbynForge\objstore\<slug>.env` |
| Firewall | ufw or firewalld opens the S3 and console ports | same | rules `forge-objstore-<slug>-<port>` |

Health is checked on the node at `http://127.0.0.1:<s3_port>/health`.

## Where S3 calls run

Bucket and object operations run **from the control plane** (`forge-server`)
straight to the store's S3 endpoint. The agent is not in that path. For a
managed store the endpoint is `http://<endpoint_host>:<s3_port>`, where the endpoint host defaults to the address the node's agent connects from
of the first member (for a distributed store, of the first member that accepts
a connection),
so the control plane must reach the members on the S3 port.

```diagram
alt: Object storage connections
flow:
  rows:
    - steps:
        - { label: Browser, edge: HTTPS }
        - { label: forge-server, tone: accent, edge: "S3 (HTTP/HTTPS)" }
        - { label: store endpoint }
    - steps:
        - { label: forge-server, tone: accent, edge: agent protocol }
        - { label: agent, note: install / lifecycle only }
```

Uploads and downloads stream through `forge-server`. A single upload is
limited to 5 GiB, and the server runs at most 8 transfers at a time.

## Credentials

Every store's S3 key lives in the [credential vault](/docs/guides/credential-vault/)
as a vault item: the username is the access key, the secret is sealed. Items
are tagged `object-storage`.

- **Managed:** Forge generates a root credential named
  `<name> (object store root)` and links it to every member node. It is **kept after
  uninstall** so you can recover the data.
- **Adopted:** the App Store container's own key, moved into the vault as
  `<name> (object store root)`.
- **External:** the key you enter when connecting. Forge tests it first and
  saves nothing if it fails. It is deleted when you disconnect.
- **Access keys** that Forge issues for services. See
  [Access keys](#access-keys).

The key never reaches the browser.

## Access keys

The root key can do everything on its store, so it should not go into
services. Instead, Forge issues each service its own **access key**,
limited to one bucket with `read` or `readwrite` access. This works only on
managed RustFS stores, including adopted ones. For a connected store, create
keys at the provider; Forge has no admin access there.

A Forge access key is a RustFS **service account** of the store's root user
with an inline IAM policy. The control plane creates it through the RustFS
admin API (`/rustfs/admin/v3/`), signed with the root key. The policy allows
`s3:ListBucket`, `s3:GetBucketLocation` and `s3:ListBucketMultipartUploads` on
the bucket, and on its objects `s3:GetObject`, plus for `readwrite`
`s3:PutObject`, `s3:DeleteObject`, `s3:AbortMultipartUpload` and
`s3:ListMultipartUploadParts`. Nothing else: no other bucket, no bucket
creation or deletion, no `ListAllMyBuckets`.

The secret goes straight into a vault item named
`<store> / <bucket> (<key name>)`, tagged `object-storage`,
`objstore:<store-id>`, `bucket:<bucket>` and `access:<access>`. These tags tell
Forge which keys it issued. The secret is never on the store page and never in
an API response. Only a platform administrator can reveal it, with
`secret.read` and an MFA check.

Issuing, rotating and revoking a key need `storage.manage` (MFA) on the store's
node; listing keys needs `storage.data`, on the node or on the environment
that [owns the store](#ownership). The store's root key cannot be revoked
through the key API.

**Rotation** proves the new key before it removes the old one: Forge creates
a new key with the same policy, lists the bucket with it, writes it into the
same vault item and only then deletes the old key. If the new key does not
work, Forge deletes it and the old key stays. The trade-off is that the old key
stops working as soon as rotation succeeds, so services must switch to the
new secret. For a switch without downtime, issue a second key, move the
service and revoke the first. **Revoking** deletes the key at the store
and its vault item.

To issue one, see [Issue object store access keys](/docs/guides/object-store-access-keys/).

## Buckets attached to services

A service rarely wants "a key". It wants a bucket it can use. Attaching
a bucket to a service does the whole job in one step: Forge issues an
access key limited to that bucket, records it for the service, and gives
the service the connection settings as environment variables:

| Variable (prefix `S3`) | Value |
| --- | --- |
| `S3_ENDPOINT` | The store's S3 endpoint, `http://<endpoint-host>:<s3-port>` |
| `S3_REGION` | The store's signing region |
| `S3_BUCKET` | The bucket |
| `S3_ACCESS_KEY_ID` | The access key Forge issued for this service |
| `S3_SECRET_ACCESS_KEY` | Its secret |

You choose the prefix, so one service can have several buckets (`MEDIA_…`,
`BACKUP_…`).

The attachment is the key's vault item with two more tags,
`app:<service-id>` and `env-prefix:<prefix>`. Nothing is copied into the
service. Each time Forge sends the service to a node (a deploy, a
restart), it reads the attachments, unseals the secrets and adds the
variables to the environment it sends. That has three consequences:

- The secret exists in one place, the vault. It is not in the service's
  settings, not in an API response and not in the audit log.
- A change takes effect with the next deploy: a new attachment, a rotated
  key, a detached bucket.
- A variable the service defines itself wins. Forge refuses to attach
  with a prefix whose variables the service already defines.

Detaching deletes the key at the store at once. The service keeps the
old variables until its next deploy, but they no longer work. Rotating an
attached key has the same edge: the old key stops working when rotation
succeeds, and the service has the new one after its next deploy.

Attaching and detaching need `deploy` on the service's environment. They
also need `storage.manage` (MFA) on the store, unless the store
[belongs to that environment](#ownership). A deploy fails when an attached store
no longer exists, so a service is never started without storage it was
given; detach the bucket to deploy again.

To attach one, see
[Attach a bucket to a service](/docs/guides/attach-bucket-to-service/).

## Bucket grants

Roles and ownership open a whole store. A **bucket grant** opens one bucket
to one person. Someone who may manage the store names a person, a bucket and
an access level, and optionally an expiry. The person then works with the
objects of that bucket without any role on the store's node, the workspace or
the owning environment.

A grant gives access **through Forge only**: the console, the REST API and
the `forge` CLI. It does not issue an S3 key. The holder gets no credential
and does not see the store's endpoint, so an S3 client cannot use a grant.
Forge makes the S3 calls itself, with the store's own credential, as it does
for everyone who browses a store. For S3 clients and services, use an
[access key](#access-keys) or an
[attachment](#buckets-attached-to-services).

| Access | Allows |
| --- | --- |
| `read` (default) | List the objects of the bucket and download them |
| `readwrite` | The same, and upload objects and delete objects |

A grant never expires unless you give it an end: a lifetime from 1 minute to
10 years, or a moment in the future. A person holds at most one live grant
per bucket. A grant cannot be edited; to change the access or the expiry you
revoke it and issue a new one.

### How a grant compares

| | Role on the node or workspace | Role on the owning environment | Bucket grant | Access key or attachment |
| --- | --- | --- | --- | --- |
| Given to | An account, by a role binding | An account, by a role binding | One account, by name | A service |
| Reaches | The whole store | The whole store | The objects of one bucket | One bucket, over S3 |
| Used through | Forge | Forge | Forge | Any S3 client |
| S3 credential | None | None | None | An access key and secret in the vault |
| Create buckets, test the connection, list keys | Yes, with `storage.data` | Yes, with `storage.data` | No | No |
| Manage the store | With `storage.manage` (MFA) | No | No | No |
| Ends | When the binding is removed | When the binding is removed | At its expiry, or when revoked | When the key is revoked |
| Works on | Managed and connected stores | Managed and connected stores | Managed and connected stores | Managed RustFS stores |

A grant is not a role. It is not part of the Cedar policy set, it is not
inherited and it does not appear among a person's role bindings. It is a
record that says "this account, this store, this bucket, this access, until
then".

### How a grant is checked

Forge asks the role rules first. Only when no role allows a request on the
objects of a bucket does it look for a grant. The request is allowed when the
caller holds a live grant for that store and that bucket with enough access.
A grant is live while it is not revoked, not expired by the server's clock,
and its holder's account is not disabled.

Forge reads the grant on every request. Revoking a grant, or its expiry,
therefore takes effect with the holder's next request. There is no session
to end and nothing to wait for. Using a grant needs no MFA step-up, like
`storage.data`.

The bucket is matched by its exact name. A grant for `uploads` does not
cover `Uploads` or `uploads-archive`, and it covers the whole bucket, not a
folder in it.

### What the holder sees

A person whose only access is a grant sees the store in the list, in a
reduced view that goes further than the one for an
[owning environment](#ownership): the store's name, provider, mode, topology
and status. The endpoint, the member nodes and the owning environment are
left out, because the holder works through Forge alone. The store's bucket
list shows only the buckets the person holds a live grant for.

### What a grant never opens

Everything outside the objects of the granted bucket stays closed:

- Other buckets of the store, and creating or deleting buckets.
- Access keys: listing, issuing, rotating and revoking.
- The connection test and the lifecycle actions.
- Attaching a bucket to a service, and changing the store's environment.
- The grants themselves. A holder cannot see who else has access and cannot
  pass the grant on.

Granting, revoking and listing a store's grants need `storage.manage` (MFA)
on the store's node or workspace. A role on the owning environment is not
enough. Every account can list its own live grants: the store, the bucket, the
access, the expiry and the status, without the note or who granted it.

### API keys, service accounts and AI connectors

A grant belongs to an account, and it counts for the non-interactive ways
that account signs in: its API keys, and the tokens of a service account that
holds the grant. This matches how tunnel grants count for API keys.

A grant does **not** count for an AI connector (MCP). A connector gets what
its user's roles allow and nothing that an administrator handed to the person
this way. That also holds when the person approves a request the connector
made: the approved request runs with the person's roles, not with their
grants. For a connector, the list of own grants is empty.

### Grants and the life of a store

Removing a store (an uninstall that removes it, or a disconnect) deletes its
grants. A grant names its bucket by name, so the grants of a bucket end with
the bucket: deleting a bucket through Forge revokes them, and creating a
bucket through Forge revokes any grant still live for that name. A new bucket
therefore never starts open to the holders of an old one. Forge does not see
a bucket that is deleted and created again outside it, in the store's own
console or with an S3 client; a grant that is still live then opens the new
bucket, so revoke the grants when you delete a bucket that way.

A grant is checked when a request starts. A revocation or an expiry refuses
the next request; a download that is already running finishes.

When the store fails, a grant holder is told only that the store can't be
reached, or that the bucket or object does not exist. The store's address
and its own error text are for the people who run it.

### Audit

Issuing and revoking are audited as `storage.grant.create` and
`storage.grant.revoke`, with the person, the bucket and the access. Every
download, upload and delete carries `grant_id`: the grant that allowed it, or
`null` when a role did. A refused request is audited as a denied
`storage.data` with the store, the bucket and the access it needed. Listing
objects is not audited, through a grant or through a role.

To grant access, see
[Grant a person access to a bucket](/docs/guides/grant-bucket-access/).

## Adoption

The App Store `rustfs` package runs RustFS as the Docker container
`forge-rustfs` with its data in `/var/lib/forge/pkgs/rustfs`. **Adoption**
turns that container into a managed standalone store without recreating it:
the container, its data, its ports and its key stay the same. Forge reads the
key from `/etc/forge/pkgs/rustfs.env` once, seals it in the vault as the store's
root key and removes the App Store's installation record. The agent takes the
key out of the job output and streams no live log for that job, so it never
reaches job history. Adoption is Linux and standalone Docker only, an adopted
store cannot be reinstalled or repaired, and adoption cannot be undone. See
[Adopt an App Store RustFS container](/docs/guides/adopt-app-store-rustfs/).

## What uninstall keeps

Uninstalling a managed store removes the service or container, the binaries,
the settings file and the firewall rules. For an adopted store it removes the
container `forge-rustfs` and `/etc/forge/pkgs/rustfs.env`. It **never deletes
the data path**, and the root credential stays in the vault, so you can recover
the objects with that key.

## Security notes

:::caution
Managed endpoints use plain HTTP for now. Run them only on private networks
that you trust, and keep the S3 port closed to the internet. TLS on managed
endpoints is not yet supported.
:::

- `storage.manage` runs scripts as root on the node, so it requires an
  MFA-verified session. It is checked on the member node for managed stores
  and on the workspace for external stores.
- `storage.data` covers buckets and objects and does not need MFA. For a
  store with an owner it is also accepted on the owning environment. See
  [Ownership](#ownership).
- A [bucket grant](#bucket-grants) opens the objects of one bucket to one
  account without a role and without MFA. With `readwrite` its holder can
  delete every object in that bucket. Review a store's grants regularly, and
  give grants an expiry where the need is temporary.
- Every mutation is audited, including uploads, downloads and deletes. See
  [Audit](/docs/concepts/audit/).

## Not yet supported

- Adding nodes or pools to a distributed store, and converting a standalone store to distributed.
- Access keys for connected stores, keys for several buckets, and custom key policies.
- Attaching a bucket of a connected store to a service.
- A store owned by several environments, and a store owned by a service
  directly. See [Ownership](#ownership).
- A personal S3 key for the holder of a bucket grant, grants for a prefix or
  folder of a bucket, and grants to a group. See
  [Bucket grants](#bucket-grants).
- Multipart uploads (objects over 5 GiB) and presigned transfers.
- Versioning, lifecycle rules and CORS.
- Single sign-on to the RustFS console.
- NAS volumes.
- TLS on managed endpoints.
