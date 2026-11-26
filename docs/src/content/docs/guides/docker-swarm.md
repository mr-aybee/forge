---
title: Run a Docker Swarm cluster
description: Create a Swarm cluster, turn its nodes into a Docker Swarm, and manage managers, workers, drain and labels from Forge.
sidebar:
  order: 7
---

This guide shows you how to create a cluster in Docker Swarm mode, add nodes
to its swarm, and manage those nodes. For how the two cluster modes differ,
see [Cluster modes](/docs/concepts/clusters/#cluster-modes).

## Before you begin

- You need the `swarm.manage` action on the cluster. The **Workspace admin**
  and **Platform admin** roles have it. It is MFA-guarded, so Forge asks you to
  confirm with your authenticator.
- To start a Docker Engine install on a node that has none, you also need
  `package.manage`.
- Nodes must run Linux (Debian/Ubuntu or a RHEL-like distribution). Windows
  nodes cannot join a swarm.
- Nodes must reach each other on TCP 2377 (management), TCP and UDP 7946
  (node communication), and UDP 4789 (overlay traffic).

## Steps

1. Go to **Clusters** → **New cluster** → **Swarm**. Enter a name, then
   select **Create cluster**. You cannot change the mode later.
2. Assign your Linux nodes to the cluster: on each node, open **Settings** →
   **Cluster**, or use `PATCH /servers/{id}` with `cluster_id`.
3. Open the cluster and select the **Swarm** tab. Select **Create swarm**.
4. Pick the first node. It becomes the swarm's first manager. Optionally set:
   - **Advertise address**: the IP address or interface other nodes reach it
     on. Leave it empty when the node has one address.
   - **Address pools**: CIDR ranges for overlay networks.
   - **Autolock**: locks manager keys at rest. After a Docker restart you must
     unlock the swarm on that node.

   Then select **Add node**.
5. Wait for the member to show **joined**. If the node had no Docker Engine,
   Forge installs it first and then joins the node.
6. Add the other nodes with **Add node**, as **manager** or **worker**.
   For a swarm that survives a manager failure, run 3 managers (or 5 to
   survive two).

:::tip
You can join a node while you assign it: send `"swarm_role": "worker"` (or
`"manager"`) with `cluster_id` in `PATCH /servers/{id}`.
:::

## Manage nodes

Use the row menu of a member on the **Swarm** tab:

| Action | Effect |
| --- | --- |
| **Promote** / **Demote** | Makes a worker a manager, or the other way round. The last manager cannot be demoted |
| **Drain** | Moves the node's tasks to other nodes and schedules no new ones. Use it before maintenance |
| **Pause** | Keeps running tasks and schedules no new ones |
| **Activate** | Schedules tasks on the node again |
| **Edit labels** | Sets node labels that placement constraints can use, such as `zone=a` |
| **Leave** | Drains the node, makes it leave the swarm and removes it from the node list |

**Rotate join tokens** issues new worker and manager tokens. Nodes already in
the swarm keep working; only new joins need the new token, and Forge keeps it
for you.

:::caution
A node in the swarm cannot move to another cluster. Make it **Leave** first.
The last manager can leave only when no other node is left.
:::

## Run a service in the swarm

In a Swarm cluster, a service runs as a Docker Swarm service. The swarm
places its tasks, so you do not pick nodes.

1. Open **Services → New service** and choose an environment of the Swarm
   cluster. **Systemd** is unavailable: a swarm runs containers only.
2. Fill in **Image**, **Ports** and the other sections as usual.
3. In **Swarm**, optionally set **Docker service name** (lowercase letters,
   digits and `-`, up to 61 characters). Empty means `<environment>-<service>`,
   for example `prod-postgres`. Choose **Replicated** with a replica count, or **Global** for
   one task on every matching node. Under **Run on**, select **Rule** to add a
   placement rule (for example **Node role** `=` `worker`, or **Node label**
   `zone` `=` `a`). Under **Spread over**, select **Label** to spread tasks
   over a node label. Then set **Max tasks per node** and **Published ports**
   (**Ingress mesh** or **Host**). **Rollout** and **Restart policy** hold
   the update and restart settings; the defaults update one task at a time
   and pause on failure.
4. Select **Create and deploy**. Forge starts a `swarm.service` job on a
   manager.

The service's **Overview** shows a **Swarm** panel instead of the node list:

| Item | Shows or does |
| --- | --- |
| Status line | One dot per replica (green: running; orange, pulsing: starting or being replaced; red: failed or missing) with running/desired, the rollout state and message, the image |
| Replica dots | Select them for each replica's node and state, and to scale with **−** / **+** and **Apply** |
| **Deploy** | Creates or updates the Docker service from the saved definition |
| **Scale** | Changes the task count now and saves it as the service's replicas, so later deploys (a webhook, a redeploy) keep it. A global service cannot be scaled |
| **Restart** | Replaces every task, following the rollout settings |
| **Roll back** | Returns the Docker service to its previous spec |
| **Remove** | Deletes the Docker service; the service is down until the next **Deploy** |
| Tasks | Slot, node, state, desired state, error and last change of each task |
| **Logs** | The last 100 to 1000 lines of all tasks, with **Refresh** |

:::caution
**Remove** stops every task. You must remove a service from the swarm before
you can delete it.
:::

:::caution[Secrets and host folders in a swarm]
- Environment variables become part of the Swarm service: every manager
  stores them, and `docker service inspect` shows them. For variables marked
  secret, choose **As files (Swarm secrets)** under **Swarm → Secret
  variables** (`swarm.secret_env: files`): Forge then puts each one in a
  Swarm secret mounted at `/run/secrets/<KEY>` and sets `<KEY>_FILE` to that
  path. Your application must read the file.
- A host folder mounted into a Swarm service must lie inside an approved
  file root on **every** node of the swarm, because a task may run on any
  of them. Other host folders need `stack.privileged` (an administrator with
  MFA). System paths, the Docker socket and Forge's own folders are always
  refused.
:::

:::note
Docker names each task container `<service-name>.<replica>.<task-id>`, for
example `prod-postgres.1.emcnoltw17figl697yaizgvc7`. The task ID is random
and changes whenever Swarm replaces the task. Docker cannot rename a running
service, so a changed **Docker service name** applies only after **Remove**
and **Deploy**. Services created before names were configurable keep the
name `forge-<service-id>`.

Forge finds its services by the `forge.service.id` label, not by name, and
only changes Swarm services it created for that service. Each environment
gets its own encrypted overlay network that other containers cannot join.
In **Projects**, a Swarm service shows a `swarm` tag, its running tasks and
the node each one runs on.
:::

## Deploy releases with a strategy

Release deployments (**Deployments**, or `forge deploy apply`) work in Swarm
clusters too. Forge plans the steps for the swarm instead of for single nodes:

| Strategy | What happens in the swarm |
| --- | --- |
| `rolling` | Swarm updates `batchSize` tasks at a time and rolls back on failure |
| `recreate` | Swarm stops every task, then starts the new release (brief downtime) |
| `blue-green` | The new release runs as a second service on its own ports; once healthy, the site sends all traffic to it; the old one stays at zero tasks for a fast rollback |
| `canary` | The new release grows step by step while the site sends it the same share of traffic |
| `manual` | Not available on Swarm. Use `rolling` with `approval: true`, or `canary` |

`blue-green` and `canary` need a site in front of the service, so the edge
can move traffic between the two versions.

Ports 20000 to 29999 are reserved for these two versions: a service in a
Swarm cluster cannot publish a port in that range.

If a rollout fails or you abort it, Forge puts the release that served
before back in place. For `blue-green` and `canary` that means the old
version is scaled up, checked, and given all traffic again; once all traffic
had moved to the new version, Forge leaves it there.

The service page shows an **HA readiness** score: more than one task, tasks
spread over nodes, a health check, restarts, rollback on failure, and at
least three managers.

## Choose the advertise address

When you add a node, the **Advertise address** field lists the node's
addresses with the best one selected:

| Source | When it is offered |
| --- | --- |
| **mesh** | The node is in a Forge network fabric (WireGuard). Preferred: the swarm then works across NAT and sites |
| **primary** | The node's main address |
| **interface** | Every other interface, except loopback, link-local and Docker bridges |
| **agent** | The address the agent connects from |

Forge also uses an IP advertise address as the data-path address, so overlay
traffic takes the same path. From the CLI: `forge swarm addresses <cluster> <node>`.

## Deploy a compose stack to the swarm

A compose stack in an environment of a Swarm cluster runs as one Docker
stack (`docker stack deploy` on a manager), not per node.

1. Open **Clusters** → your cluster → **Swarm** → **Stacks** and select
   **Deploy stack**, or run `forge stacks create` with an environment
   of the Swarm cluster and a manager as `--node`.
2. Paste or upload the compose file. Forge checks it with the compose policy
   and these Swarm rules:
   - `secrets:` and `configs:` must be declared `external: true`; create them
     first under **Secrets & configs**. Forge mounts the newest version.
   - `network_mode` and relative bind mounts (`./data`) are refused. Host
     folders must be inside a root approved on every node.
   - `depends_on` and `container_name` are dropped: Swarm ignores them. Give
     services a health check and a restart policy instead.
3. Select **Deploy**. Forge starts a `swarm.stack` job and waits until every
   service runs its tasks.

Services that publish `ports:` (or set `x-forge: { shared: true }`) also join
the shared network `forge-shared`, so services of other stacks reach them by
service name (`<namespace>_<service>`, for example `forge-prod-shop_api`).
Services without ports stay on the stack's own encrypted overlay and are
reachable only inside the stack.

```yaml
services:
  api:
    image: registry.example/api:1.4
    ports: ["8080:80"]
    secrets: [db_password]
    deploy: { replicas: 2 }
  redis:
    image: redis:7
secrets:
  db_password: { external: true }
```

Redeploy with a new revision for a rolling update, or deploy an older
revision to roll back. Per service you can **Scale**, **Restart**, **Roll
back** and read **Logs**; the stack view shows replicas, published ports,
every task with its errors, and CPU and memory of running tasks.

## Manage secrets and configs

Under **Swarm → Secrets & configs**, **Create** a secret or config from a
value or a credential vault item. Values never leave the request: Forge does
not store, log or return them. **Rotate** creates the next version
(`<name>.v2`); redeploy the stacks and services that use it. **Prune**
removes old versions no service uses. This needs `swarm.manage` (MFA).

A single Swarm service mounts secrets and configs in **Swarm → Secrets &
configs** of the service editor, with the file name, owner and mode.

## Networks and volumes

**Swarm → Networks** lists the swarm's overlays with subnets, encryption and
attached services. `ingress` is the routing mesh: a published port answers on
every node. **Create forge-shared** makes the shared network if no stack has
yet.

**Swarm → Volumes** lists each node's volumes and the containers using them,
and creates local, NFS or CIFS volumes. A local volume exists only on its
node: pin a stateful service to that node with a placement rule
(`node.hostname == <node-name>` or a node label), or use an NFS or CIFS volume
that every node can mount.

## Send traffic to a stack service

In an ingress endpoint, add a **Swarm port** target: choose the cluster and
the stack service's published port. Forge sends traffic to every joined
swarm node on that port, and the routing mesh forwards it to a task. From the
CLI, use a site spec with the target
`{"kind": "swarm", "cluster_id": "<cluster-id>", "port": 8080, "service": "shop api"}`
and `forge sites create --file site.json`.

## Convert a Default cluster to Swarm

On a Default cluster select **Convert to Swarm** (or `forge swarm convert
<cluster> --to swarm --check`). Forge converts only when:

- every node runs Linux;
- no service of the cluster's environments runs on chosen nodes;
- no compose stack is deployed per node.

Then create the swarm as above. Converting back to Default needs every node
to have left the swarm and no Swarm service left: Forge does not move running
Swarm tasks onto nodes for you. Remove the stacks and services, make each
node **Leave**, then convert.

## Set up a swarm from the CLI

```sh
forge clusters create --file cluster.json          # {"name": "swarm-eu", "mode": "swarm"}
forge nodes enroll-token --cluster swarm-eu --hostname lab-a   # one token per node; install with it
forge swarm init swarm-eu lab-a --wait
forge swarm join swarm-eu lab-b --role worker --wait
forge swarm join swarm-eu lab-c --role worker --wait
forge swarm labels swarm-eu lab-c --label zone=b
printf '%s' "$DB_PASSWORD" | forge swarm secrets create swarm-eu db_password --value-file -
forge swarm networks shared swarm-eu
forge stacks create shop --environment prod --node lab-a --compose compose.yaml
forge swarm stacks deploy shop --wait --yes
forge swarm stacks ps shop
forge swarm stacks scale shop api 3
forge swarm stats shop
forge swarm stacks logs shop api --tail 100
forge swarm drain swarm-eu lab-b
```

## Verify

- The **Swarm** tab lists every node as **joined**, and the node cards show
  **ready** with the right role.
- The manager count shows how many manager failures the swarm tolerates.
- On a manager node, `docker node ls` lists the same nodes.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| The member stays **joining** | Docker Engine is still installing, or the node is offline | Check the install job on the node, and that the node is online |
| **failed**: `the node is already in another Docker Swarm` | The node joined a swarm outside Forge | Run `docker swarm leave --force` on the node, then add it again |
| **failed**: join timed out | Firewall blocks TCP 2377, TCP/UDP 7946 or UDP 4789 between nodes | Open the ports, then **Leave** with **Force** and add the node again |
| `no swarm manager answered` | Every manager is offline | Bring a manager online; the live node list comes back on its own |
| `the swarm is still being created` | The first node has not joined yet | Wait until the first node shows **joined**, then add more nodes |
| Stack deploy: `secrets \`x\` does not exist in this swarm` | The stack names a secret Forge has not created | Create it under **Secrets & configs**, then deploy again |
| Stack deploy: `... create it in Forge ... and declare it external: true` | The compose file uses `file:` secrets | Create the secret in Forge and declare it `external: true` |
| A stack service cannot reach another stack's service | The target publishes no port, so it is not on `forge-shared` | Publish a port or set `x-forge: { shared: true }`, and use `<namespace>_<service>` |
| Overlay traffic fails between sites | Nodes advertise LAN addresses | Put the nodes in a network fabric and re-join them with the mesh address |
| `docker stack deploy failed` | The Docker CLI on the manager is missing or old | Install Docker Engine from the App Store on the manager |

## Next steps

- [Cluster modes](/docs/concepts/clusters/#cluster-modes)
- [REST API: Docker Swarm](/docs/reference/rest-api/#docker-swarm)
- [Deploy a compose stack](/docs/guides/compose-stacks/)
