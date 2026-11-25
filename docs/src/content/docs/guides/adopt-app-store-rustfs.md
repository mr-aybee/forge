---
title: Adopt a RustFS container
description: Turn a RustFS container that already runs on a node, from the App Store or created outside Forge, into a managed object store without recreating it or moving its data.
sidebar:
  order: 11
---

This guide hands a RustFS container that you installed from the App Store
`rustfs` package over to **Object Storage**. Forge then manages it as a
standalone object store, with the Object Explorer, lifecycle buttons and
[access keys](/docs/guides/object-store-access-keys/). The container, its data and
its credential stay the same. For the model, see
[Adoption](/docs/concepts/object-storage/#adoption).

To adopt a RustFS container that you created yourself, see
[Adopt any RustFS container](#adopt-any-rustfs-container).

## Before you begin

- You need the `storage.manage` action on the node: role **Operator**,
  **Admin** or **Platform admin**.
- `storage.manage` requires an MFA-verified session. See [MFA](/docs/guides/mfa/).
  In the console, Forge asks for your authenticator code when needed. The API
  example keeps the browser session cookie in `cookies.txt` and the CSRF token
  in `$CSRF`.
- The node runs Linux and has the App Store **RustFS** package installed and
  running: container `forge-rustfs` on Docker Engine, settings in
  `/etc/forge/pkgs/rustfs.env`. If the container is stopped, adoption starts
  it.
- The node is online, and the control plane reaches it on the RustFS S3 port
  (the App Store default is `9000`).
- No other object store on the node uses the instance name `rustfs` or the
  same S3 port.
- To assign the store to an environment, you also need `deploy` on that
  environment (role **Developer**, **Operator**, **Admin** or
  **Platform admin**). See [Ownership](/docs/concepts/object-storage/#ownership).

## What adoption does

| Item | After adoption |
| --- | --- |
| Container | Same container `forge-rustfs`. Forge does not recreate or restart a running container |
| Data | Stays in `/var/lib/forge/pkgs/rustfs` |
| Ports | The S3 and console ports of the App Store installation (`port`, `console_port`; defaults `9000` and `9001`) |
| Store | Standalone, runtime `docker`, instance name `rustfs`, release from the installation |
| Root key | Read from `/etc/forge/pkgs/rustfs.env` and sealed in the credential vault as `<name> (object store root)`. The agent takes it out of the job output and sends no live log for this job, so the key never appears in job history |
| App Store | The `rustfs` installation record is removed. The App Store no longer lists or manages the container |

## Steps

1. Open **Object Storage** and select **Adopt App Store RustFS**.
2. In **Node**, choose the node. The list shows every node with an App Store
   RustFS installation, with its release. Nodes that are offline, run Windows
   or are already adopted are listed but cannot be chosen; the reason is shown
   next to the node.
3. Enter a **Name** for the store, for example `Media`.
4. Optional: in **Endpoint host**, enter the name or address the control plane
   uses to reach the node. Leave it empty to use the node's hostname. The hint
   below the field shows the resulting S3 endpoint.
5. Optional: in **Environment**, choose the environment the store belongs to.
   Leave it on **Shared (no environment)** for a shared store.
6. Select **Adopt**. The dialog shows the `storage.action` job log. The store
   has status `adopting` until the job finishes. Select **Open store** to go to
   the store page.

### With the REST API

Send the adoption request. You need the node's server id
(`GET /api/v1/servers`). `endpoint_host` is optional. So is `environment_id`,
which assigns the store to an environment.

```sh
curl -sS -b cookies.txt -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -X POST https://<forge-host>/api/v1/object-stores/adopt \
  -d '{"name": "Media", "server_id": "<server-id>", "endpoint_host": "<node-host>"}'
```

The response is `202 {store, job}`. The store has status `adopting`. Wait for
the `storage.action` job to finish on **Operations → Jobs** or on the store page. It checks
the container, waits for its health endpoint and reports the key once.

To change or clear the store's environment later, see
[Assign a store to an environment](/docs/guides/install-object-store/#assign-a-store-to-an-environment).

## Adopt any RustFS container

Forge finds every RustFS container on your Linux nodes: image
`rustfs/rustfs` from any registry with any tag, or a container with Forge's
label `org.orbyn.forge.objstore`.

Before you begin, you also need `server.manage` on the node, because the scan
runs `docker` commands there and adoption reads the root credential. Forge scans only the nodes you may manage. The
container must keep its data on a bind mount or a named volume.

1. Open **Object Storage**. The **Found on nodes** section lists the
   containers and, per node, whether Docker is available. Select
   **Scan again** to refresh it.
2. Each container shows its kind: **managed** (a Forge store runs it),
   **orphaned** (Forge's label but no store, for example after the store
   record was removed) or **foreign** (created outside Forge). Adopt an
   orphaned or foreign container.
3. Enter a **Name** and, optionally, an endpoint host and environment, then
   adopt. The endpoint host must be the node's own address or host name
   (`400` otherwise). On a container with the host network, Forge cannot read the ports;
   enter the S3 port and console port.

With the REST API, list the containers, then adopt one by name:

```sh
curl -sS -b cookies.txt https://<forge-host>/api/v1/object-stores/discover
curl -sS -b cookies.txt -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -X POST https://<forge-host>/api/v1/object-stores/adopt \
  -d '{"name": "Media", "server_id": "<server-id>", "container": "<container-name>"}'
```

What changes:

| Item | After adoption |
| --- | --- |
| Container | Same container, name, network and mounts. Forge does not recreate it |
| Data | Stays where it is. Each data mount becomes a drive of the store; a named volume stays a named volume |
| Root key | Read from the container's environment on the node and sealed in the vault. The agent strips it from the job output. A container that sets no credential uses the engine default `rustfsadmin`; the job log warns you, and you should change it |
| Store | Standalone, runtime `docker` |

An adopted container keeps the configuration it was found with: it is never
reinstalled and its settings cannot be edited. Removing the store keeps the
container running and its data in place; Forge only forgets it. Engine
variables that fail Forge's validation are dropped from the store's record.

## Verify

- The store shows status `running` and the **Adopted** badge on **Object Storage**.
- The job log ends with `adopted forge-rustfs (data in /var/lib/forge/pkgs/rustfs)`.
  It contains no access key or secret.
- **Test connection** on the store page reports the latency and bucket count,
  and the Object Explorer lists the buckets the container already had.
- The item `<name> (object store root)` exists under
  **Administration → Credential vault**.
- The **App Store** no longer lists the RustFS installation on the node.

## Limits

- An adopted store cannot be reinstalled or repaired. **Repair** is disabled
  on the store page, and the API refuses the `install` action with
  `an adopted App Store container is kept as it is; it cannot be reinstalled`.
  **Start**, **Stop**, **Restart** and **Refresh status** work.
- The store stays standalone on Docker. It cannot become distributed or move
  to the native runtime.
- **Uninstall** removes the container `forge-rustfs`, the file
  `/etc/forge/pkgs/rustfs.env`, and closes its ports in ufw or firewalld. It keeps
  the data in `/var/lib/forge/pkgs/rustfs`, and the root key stays in the
  vault.
- Adoption cannot be undone. To return to the App Store, uninstall the store
  and install the package again.

:::caution
After **Uninstall**, the root key exists only in the vault. Keep the vault item
if you want to reach the kept data again with that key.
:::

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `404` with `no App Store RustFS installation on this node` | The App Store has no `rustfs` installation on that node | Check the node in the **App Store**. To create a new store instead, see [Install an object store](/docs/guides/install-object-store/) |
| The dialog says `No node has an App Store RustFS installation` | No node has the App Store `rustfs` package | Install a new store with **Install RustFS** instead. See [Install an object store](/docs/guides/install-object-store/) |
| `409` with `already adopted` | A store already uses this container | Use the existing store on **Object Storage** |
| `409` with `uses the name rustfs or that port` | Another object store on the node has the instance name `rustfs` or the same S3 port | Uninstall or move that store first |
| Job fails with `there is no App Store RustFS container (forge-rustfs)` | The container was removed outside Forge | Select **Uninstall** on the failed store first (it also deletes `/etc/forge/pkgs/rustfs.env`). Then [install a new object store](/docs/guides/install-object-store/) |
| Job fails with `the App Store RustFS settings (/etc/forge/pkgs/rustfs.env) are missing` or `keys are missing` | The settings file was deleted or edited | Restore the file from a backup. Forge needs `FORGE_PKG_RUSTFS_ACCESS_KEY` and `FORGE_PKG_RUSTFS_SECRET_KEY` in it |
| Job fails waiting for the health endpoint | The container does not start or answer on the S3 port | On the node, run `docker logs forge-rustfs` and `curl -fsS http://127.0.0.1:<s3-port>/health` |
| Store status `failed` after an `install` action through the API | Repair is refused for adopted stores | Select **Refresh status** to update the status |
| `403 step_up_required` | Your session is not MFA-verified | Confirm your authenticator code, then retry |
| A node shows Docker `not_installed`, `unreachable` or `unsupported_os` | Docker is missing, the node or Docker does not answer, or the node runs Windows | Install Docker Engine from the App Store, bring the node online, or use a Linux node |
| `400` with `the container has no data mount` | The container keeps its data inside its own filesystem | Recreate it with a bind mount or named volume, or install a new store |
| `409` with `that container already belongs to an object store` | A store already runs it | Use that store |
| Job log warns `sets no credential` | The container uses the engine's default credential | Change the root credential in RustFS and update the vault item |
| `404` with `environment not found` | The environment you chose does not exist in this workspace | Choose an existing environment, or leave the store shared |
| `403` although you have `storage.manage` on the node | You chose an environment where you lack `deploy` | Ask for a role with `deploy` on it, or adopt the store as shared and assign it later |

## Next steps

- [Browse buckets and objects](/docs/guides/browse-objects/)
- [Issue object store access keys](/docs/guides/object-store-access-keys/)
- [REST API: object storage](/docs/reference/rest-api/#object-storage)
