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
- Environment variables, including ones marked secret, become part of the
  Swarm service. Every manager stores them, and `docker service inspect` on a
  manager shows them. Restrict who can log in to manager nodes.
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

## Next steps

- [Cluster modes](/docs/concepts/clusters/#cluster-modes)
- [REST API: Docker Swarm](/docs/reference/rest-api/#docker-swarm)
- [Deploy a compose stack](/docs/guides/compose-stacks/)
