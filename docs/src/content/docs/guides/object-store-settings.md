---
title: Change object store settings
description: Change the release, ports, region and container options of a managed RustFS store without losing its data.
sidebar:
  order: 11
---

This guide changes the release, ports, region or container options of a
managed RustFS store. Forge applies the change on every member node, keeps
the data, and on Docker puts the previous container back if the new one does
not become healthy.

## Before you begin

- You need the `storage.manage` action on the store's node: role
  **Operator**, **Admin** or **Platform admin**. It requires an MFA-verified
  session (see [MFA](/docs/guides/mfa/)).
- The store is a managed RustFS store and no action is running on it.
  Connected stores and adopted containers have no editable
  settings; the dialog shows the reason.
- A new port is 1024 or higher and free on every member node. The console
  port differs from the S3 port.
- For a new release on Docker, the node can pull the image.
- An image other than `rustfs/rustfs` also needs `server.manage` on every
  member node.

:::caution
S3 clients lose access while the store restarts. A port change also changes
the store's endpoint, so update the clients that use it.
:::

## Steps

1. Open **Object Storage**, then the store, and select **Settings**.
2. Change the **Release**, **S3 port**, **Console port** or **Region**.
3. For a Docker store, change the image, restart policy, memory and CPU
   limits or extra RustFS variables. The container name, network and volumes
   are fixed after install and cannot be changed here.
4. Save. Forge starts one `storage.action` job per member node.

With the REST API, send the full desired state:

```sh
curl -sS -b cookies.txt -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -X PUT https://<forge-host>/api/v1/object-stores/<store-id>/settings \
  -d '{"version": "1.0.6", "s3_port": 9000, "console_port": 9001, "region": "us-east-1",
       "docker": {"image": "rustfs/rustfs", "restart": "unless-stopped", "memory_mb": 4096, "env": {}}}'
```

Read the current state first with `GET /api/v1/object-stores/<store-id>/settings`.
Leave out `docker` for a native store. See
[Store settings](/docs/reference/rest-api/#store-settings) for every field.

### What the job does

| Runtime | Steps |
| --- | --- |
| Docker | Pulls the new image first. Renames the running container to `<name>-forge-previous` and stops it. Starts the new container on the same data. Waits up to 90 seconds for the health endpoint. On success, removes the previous container. On failure, removes the new container, restores the previous one and its env file, and the job fails with the reason |
| Native | Runs the install again in place with the new settings. Data and the root credential are kept |

When the job fails, Forge also restores the store's previous release, ports
and region.

## Verify

- The job on **Jobs** succeeds on every member and the store shows `running`.
- **Settings** shows the new values.
- **Test connection** on the store page succeeds.
- For Docker, on the node: `docker inspect <container-name> --format '{{.Config.Image}}'`
  shows the new image and tag.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `409` with `an action is running on this store` | Another job runs on the store | Wait for it to end, then save again |
| `409` with `another object store on a member node uses that port` | The port is taken by another store | Choose another port |
| `400` with `variable <name> cannot be set` | The variable is not on the allowed list | Use one of the variables listed in [Docker settings](/docs/reference/rest-api/#docker-settings) |
| `400` with `container settings apply to Docker stores only` | `docker` was sent for a native store | Leave out `docker` |
| The job fails and the old container runs again | The new container did not become healthy within 90 seconds, or the image could not be pulled | Read the job log, fix the cause (image name, release, variable value) and save again |
| **Settings** is disabled, or `409` | The store is connected, is an adopted container (it keeps the configuration it was found with), or has a job running | Nothing to change, or wait for the job |

## Next steps

- [Install an object store](/docs/guides/install-object-store/)
- [Issue object store access keys](/docs/guides/object-store-access-keys/)
- [REST API: store settings](/docs/reference/rest-api/#store-settings)
