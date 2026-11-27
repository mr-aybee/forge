---
title: Adopt a container as a service
description: Turn a container that already runs on a node into a Forge service without recreating or restarting it.
sidebar:
  order: 8
---

You can take a container that was started outside Forge, by `docker run` or
another tool, and manage it as a Forge `container` service. Forge reads the
container's settings and records it as running. It does not touch the
container until you deploy the service.

## Before you begin

- `deploy` on the environment (roles `developer` and up).
- `server.manage` and `container.inspect_env` on the node, with an
  MFA-verified session (roles `operator` and up). Forge reads the
  container's environment values, which is why `container.inspect_env` is
  needed.
- A Linux node with Docker. Windows nodes and native (systemd) services are
  not supported.
- An agent from this release or later on the node. It reports the
  `adopted_containers` fact. Older agents get `409 agent_update_required`:
  [update the agent](/docs/guides/update-agents/) first.
- The container must not be named `forge-app-…`. Such containers are
  already Forge services, so Forge refuses them.

## What Forge reads

| Read | How it is stored |
| --- | --- |
| Image | `image` |
| Environment | Plain variables. Variables whose names contain `KEY`, `SECRET`, `PASSWORD`, `TOKEN` or `CREDENTIAL` are sealed as secret variables. `PATH` is skipped |
| Published ports | `ports` |
| Bind mounts and named volumes | `volumes` |
| Restart policy, user, hostname | `restart_policy`, `user`, `hostname` |
| Labels | `labels`, without Docker and Compose labels |

Forge does not read an entrypoint override. Check `entrypoint` on the
service after adoption if the container uses one.

## Steps

1. Find the container name on the node, under **Containers**, or with
   `docker ps` on the node.

2. Adopt it. In the console, open **Containers**, open the row menu of the
   container and choose **Adopt as service**. Pick the environment and,
   optionally, a service name. From the CLI:

   ```sh
   forge services adopt --environment <environment> --node <node-name> --name <service-name> <container-name>
   ```

   Without `--name`, the service is named after the container. The API call
   is `POST /api/v1/services/adopt` with
   `{"environment_id": "<id>", "server_id": "<id>", "container": "<container-name>", "name": "<service-name>"}`.
   It answers `201` with `{service, instance}`.

3. Review the new service's definition, for example its secret variables
   and entrypoint:

   ```sh
   forge services show <service-name> --environment <environment>
   ```

4. When you are ready, deploy the service. This is the only step that
   replaces the container:

   ```sh
   forge services deploy <service-name> --environment <environment> --wait --yes
   ```

## What happens after adoption

- The service records `adopted_from`, the original container name, and one
  instance on that node, marked deployed and running.
- Until the first deploy, **Start**, **Stop**, **Restart**, status and logs
  act on the adopted container.
- The service reconciler leaves an adopted service alone. It is not
  restarted or moved to another node.
- The first deploy stops the adopted container and creates the normal
  `forge-app-<environment>-<service>` container. Once the new container
  runs, Forge removes the old one. If the new container fails to start,
  Forge starts the old one again. After a successful deploy, `adopted_from`
  is cleared.
- Named volumes and host paths are reused by the new container. Deleting a
  service never deletes volumes or host paths.

:::caution
The first deploy restarts the workload: the old container stops before the
new one starts. Plan a short downtime.
:::

The action is audited as `service.adopt`.

## Verify

```sh
# The service, with adopted_from set and one running instance
forge -o json services show <service-name> --environment <environment>
```

The container keeps running under its old name until the first deploy.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `409 agent_update_required` | The node's agent does not report `adopted_containers` | [Update the agent](/docs/guides/update-agents/), then adopt again |
| The container is refused as already a service | Its name starts with `forge-app-` | It is already a Forge service; open it under **Services** |
| `the container uses the host network` | Host networking cannot be represented as a Forge service | Run it with published ports first, or keep it unmanaged |
| `403` | `server.manage` or `container.inspect_env` missing on the node, or `deploy` missing on the environment | Ask an operator or administrator for the role |
| `403 step_up_required` | `container.inspect_env` needs an MFA-verified session | Run `forge auth step-up`, or sign in again with MFA |
| The new container does not start on the first deploy | A setting Forge does not read, such as an entrypoint override, is missing | Forge started the old container again. Add the setting to the service and deploy again |

## Upgrading from Object Storage

Forge no longer manages object stores. On upgrade, the server moves each
existing store as follows.

- **Managed Docker stores.** The server records the store's container, one
  per member node, and adopts it as a container service through the same
  path as above, as soon as that node is online. The container is not
  restarted.
  - The service goes into the store's environment, or the workspace's
    oldest environment when the store had none. A workspace without any
    environment keeps the store pending until one exists; the server logs a
    warning.
  - The service is named after the store's slug, with `-2`, `-3` and so on
    on a name clash or for further members of a distributed store.
  - Its `console` link is set to the store's console port (`http`, path
    `/`).
  - The store's root credential stays in the credential vault as an
    ordinary item, tagged `object-storage`, now linked to the service.
- **Native (systemd) stores** are not adopted. They keep running untouched
  on their node, no longer managed by Forge.
- **Connected external S3 stores** are not adopted. Their records are
  dropped.
- The vault credentials of all stores stay.
- Buckets, access keys and bucket grants are no longer managed by Forge.
  RustFS's own IAM still holds the keys; manage them in the RustFS console.

See the [upgrade notes](/docs/reference/upgrade-notes/#object-storage-removed).

## Next steps

- [Services](/docs/guides/services/)
- [Run RustFS as a service](/docs/guides/run-rustfs-service/)
- [`forge services adopt`](/docs/reference/cli/services/#forge-services-adopt)
