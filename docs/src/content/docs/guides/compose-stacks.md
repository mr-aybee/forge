---
title: Deploy a compose stack
description: Upload a docker-compose file, preview where each service runs and what the policy allows, start the services you pick, then update, roll back, expose or take the stack down.
sidebar:
  order: 6
---

This guide shows you how to run a Docker Compose file on one or more nodes:
preview it, start the services you choose, and manage the stack afterwards.
For how placement, the policy and port binding work, see
[Compose stacks](/docs/concepts/compose-stacks/).

## Before you begin

- The nodes run Linux with **Docker Engine** from the App Store, which
  includes the Compose plugin. See [Containers](/docs/guides/containers/).
- You need `stack.manage` on the environment (**Developer**, **Operator**,
  **Admin** or **Platform admin**) and `server.manage` on the nodes
  (**Operator** or higher).
- Stacks with host-access settings (privileged mode, host network, the
  Docker socket, binds outside approved folders) need `stack.privileged`:
  **Admin** or **Platform admin**, with MFA. See
  [Compose extensions and policy](/docs/reference/compose-extensions/#policy).
- Bind mounts outside the stack folder must be inside an
  [approved file root](/docs/guides/approve-file-roots/) on that node.

## Create a stack

1. Open a node, select the **Docker** tab (**Containers**), then **Create** → **Create from
   Compose**. The **New stack** page opens at `/stacks/new`, with that node and
   its environment already chosen. The list of all stacks is at `/stacks`; it is
   not in the sidebar.
2. Choose the **Node** (services without placement run there), the
   **Environment** and a **Stack name**.
3. Paste the compose file into **compose.yaml**, or select **Upload file**.
4. Optional: expand **Environment (.env) — optional** and add variables (or
   select **Upload .env**). Optional: set a **Folder** inside an approved root
   instead of `/opt/forge/stacks/<environment>/<stack>/`.
5. Check the preview on the right. It refreshes a moment after you stop
   typing; select **Refresh** to check again. Select **Preview** in the page
   header to hide or show the preview column; the console remembers your
   choice.

To run a service on another node, name it in the file:

```yaml
services:
  web:
    image: nginx:1.27
    ports: ["8080:80"]
    x-forge: { node: lab-b }
  worker:
    image: ghcr.io/example/worker:1.0
    deploy:
      placement:
        constraints: ["node.hostname == lab-c"]
```

## Check the preview

The preview groups services by node and shows, for each service, its image,
ports, volumes, variables (secret values masked), dependencies, health check
and the policy result:

- ✔ allowed;
- ⚠ needs `stack.privileged`: only an administrator with MFA can start it;
- ✘ blocked: change the file. Blocked settings, such as `build:`, are never
  allowed.

Each port shows the address it binds to. Ports bind to the node's cluster
address (or `127.0.0.1`) unless the service lists them in `x-forge.public`.

## Start services

1. Under **Services to start**, tick the services to run. Their dependencies
   are started too.
2. In the action bar at the bottom of the page, select **Save & Start**, or
   **Save** to keep revision 1 without starting anything. **Save & Start**
   stays disabled, with the reason in its tooltip, until the preview passes.

Forge runs one job per node and follows each job on the page; select
**Open stack** to go to the stack. Each service appears with
its state and health, and is listed as a service of the environment.

## Update and roll back

1. Open the stack and select **Update**.
2. Edit the compose file. Leave **.env** blank to keep the stored values.
3. Select **Save**. A changed file becomes a new revision.
4. Select **Deploy** to apply it. Tick **Pull images again** to fetch newer
   images with the same tag.

To roll back, open **Revisions**, compare a revision with the latest, and
select **Roll back to this**.

## Operate services

On the **Services** tab, each node lists its services. Use **Start**,
**Stop**, **Restart**, **Pull image** and **Logs** per service, or **Pull**,
**Restart all** and **Down** for the whole node. Select **Refresh** to read the
state from the nodes again.

## Expose a service through Nginx

1. Open the stack, open **More stack actions** and select **Expose via
   Nginx**.
2. Choose the **Service** and its **Port** (the published host port).
3. Enter the **Domains**, choose **TLS**, and tick the nodes under **Edges
   (nodes running Nginx)**.
4. Select **Create Site**, then **Open Ingress** and apply the site on its
   edges.

Without a cluster network the service listens on `127.0.0.1` only, so the
edge must be the service's own node with Nginx installed.

## Take the stack down

1. Open **More stack actions** and select **Down…**.
2. Tick **Also remove volumes (data is lost)** only if you want to delete the
   stack's named volumes.
3. Confirm. Files in a folder you chose stay; Forge's own folder is removed
   when it is empty.

To delete the stack and its revisions, take it down first, then select
**Delete…**.

## Verify

- The stack page shows each service as `running` (and `healthy` when it has
  a health check).
- **Operations → Audit log** shows `stack.create`, `stack.update`, `stack.deploy`,
  `stack.service` or `stack.down` with the stack name, revision and nodes.
- On the node: `docker compose ls` lists `forge-<environment>-<stack>`.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `no node named …` | `x-forge.node` or a constraint names an unknown host | Use the node's hostname or display name |
| `… depends on …, which runs on another node` | `depends_on` across nodes | Place both services on the same node |
| `needs stack.privileged` | A host-access setting | Ask an administrator to deploy it with MFA, or remove the setting |
| `… is outside the stack folder and the approved file roots` | A bind mount elsewhere on the host | Approve the folder as a file root, or use a named volume |
| `… lies inside the read-write bind mount …` | One bind (or `env_file`) is a subfolder of another service's writable bind | Mount separate folders, or make the outer bind read-only (`:ro`) |
| `… contains the stack folder (compose.yaml, .env)` | A read-write bind of `.` | Mount it read-only (`.:/app:ro`) or mount a subfolder such as `./app` |
| `… is a symbolic link; bind sources must be real folders` | A link on the way to a bind source on the node | Replace the link with the real folder |
| `unknown service key …` | A key that is not part of the Compose specification | Remove it, or prefix it with `x-` |
| `the Docker Compose plugin is not installed` | Docker without Compose v2 | Install **Docker Engine** from the App Store |
| The job fails with a `docker compose failed` message | An image pull or a health check failed | Read the message and the service **Logs**, fix and deploy again |
| The site returns 502 | The edge cannot reach the port | Use a cluster network, or expose from the service's own node |

## Next steps

- [Create a container](/docs/guides/create-container/)
- [Compose stacks](/docs/concepts/compose-stacks/)
- [Compose extensions and policy](/docs/reference/compose-extensions/)
- [REST API: Compose stacks](/docs/reference/rest-api/#compose-stacks)
