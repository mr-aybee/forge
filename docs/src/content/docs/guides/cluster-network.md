---
title: Cluster network
description: Give a cluster a private subnet, connect its nodes with an encrypted WireGuard mesh, and keep addresses and internal names stable.
sidebar:
  order: 7
---

This guide shows you how to create a cluster's private network, connect its
nodes with WireGuard and pin addresses. Forge manages each cluster's network
itself, without Swarm or Kubernetes.

## Before you begin

- Creating or changing the network needs `workspace.manage` on the cluster
  (**Admin** or **Platform admin**).
- **Apply network** also needs `server.command` on every member, which needs
  an MFA-verified session.
- Members run Linux, or Windows with `winget` available, and can reach each
  other, or a public member, on the WireGuard port (UDP `51820` by default).
  When firewalld runs (the default on RHEL, Rocky and Alma), the agent opens
  that port in it. Open it yourself in ufw, nftables or a cloud firewall.
- On RHEL 8 and some rebuilds, `wireguard-tools` comes from EPEL. The agent
  installs `epel-release` when the first install fails; on RHEL itself, enable
  EPEL first if that is not possible.
- For the [container network](#container-network), Linux members need Docker
  Engine 28.0 or newer and an agent that reports the `mesh_network` fact.
  Nodes without it keep the mesh and run stacks as before.

:::note[Limits]
Forge connects nodes with an encrypted WireGuard mesh only; there is no
unencrypted routed mode. Service IPs are reserved in the address plan
but not configured on nodes, and there is no firewall between environments and
clusters. Names resolve through the hosts file and, on Linux members,
through forge-dns (see [Internal names](#internal-names)).
:::

## Create the network

1. Open the cluster and select **Network** in the sidebar. You can also
   create the network in **New cluster → Default** with **Create the Forge cluster
   network now**.
2. Under **Set up the cluster network**, pick the size:
   - **Class B · /16** (default): 65,534 addresses, up to 239 nodes and 256
     services.
   - **Class C · /24**: 254 addresses, up to 10 nodes and 64 services.
3. Keep the suggested **Subnet** or type your own private range. Forge
   suggests the next free subnet from the pool (business setting
   `fabric.pool`, default `10.64.0.0/10`). It never overlaps another cluster
   or a network your nodes are on.
4. Enter the **Internal domain**, for example `lab.forge.internal`.
5. Select **Create network**.

From the command line ([`forge clusters network`](/docs/reference/cli/clusters/#cluster-network)):

```sh
# See the suggested subnet and domain
forge clusters network suggest <cluster>
# Save the plan: flags you leave out take the suggestion
forge clusters network set <cluster> --subnet 10.64.0.0/16 --domain lab.forge.internal
```

The network is always an **Encrypted mesh (WireGuard)**. It works across
clouds, data centres and home labs behind NAT. In the API, `spec.mode` is
`mesh` and may be left out; `routed` is refused with `400`.

The plan assigns addresses straight away:

| Region | /16 example `10.64.0.0/16` | /24 example `10.64.5.0/24` |
| --- | --- | --- |
| Gateway and ingress VIPs | `10.64.0.1 – .15` | `.1 – .15` |
| Node addresses | `10.64.0.16 – .254` | `.16 – .31` |
| Service IPs | `10.64.1.0 – 10.64.1.255` | `.32 – .95` |
| Instance blocks | one /24 per node from `10.64.10.0` | one /28 per node from `.96` |

Addresses are stable: a node or service keeps its address while it exists,
and a new one takes the lowest free slot. Adding a node to the cluster or
removing it updates the plan straight away.

## Apply the mesh

1. On the cluster's **Network** page, select **Apply network**.
2. If asked, confirm in **Verify it's you**.

From the command line, `--wait` waits until every node reports `ok` or
`failed` and prints the node table:

```sh
forge clusters network apply <cluster> --yes --wait
```

For each member, Forge:

1. installs WireGuard if it is missing (`wireguard-tools` on Linux,
   WireGuard for Windows through `winget`);
2. has the node create its own key pair. The private key stays on the node
   (`/etc/forge/wireguard/`, root only) and only the public key is sent;
3. writes the interface (`forge` + part of the cluster id) with the node's
   address and its peers, adds the routes, and enables it at boot.

Who connects to whom:

- Nodes with a public address, or a public endpoint you set, are reached
  directly.
- A node behind NAT (a home lab) dials out to them and keeps the tunnel open.
- Nodes on the same private network connect to each other directly.
- Two NATed sites reach each other through a relay: the first public Linux
  node.

If a node is behind NAT but has a port forward, open its row menu in
**Nodes** on the **Network** page and select **Set public endpoint**. Enter
the `host:port` the router forwards to the node's WireGuard port. From the
command line, `forge clusters network set <cluster> --endpoint <node-name>=192.0.2.10:51820`
does the same; the `--endpoint` list replaces the saved one.

A background job then keeps every node in sync. It acts as the person who
last selected **Apply network** (or **Re-apply network**), and checks their
rights again for every command. If they lose access, the nodes show the
denial and you can apply again yourself.

- It checks every node about every 20 seconds.
- A failed step is retried after 30 s, then 1, 2, 4 and 8 minutes, up to
  every 15 minutes.
- A node that was offline is picked up as soon as it reconnects.
- A node set up before some peers had keys is updated once those keys
  arrive, so one **Apply network** on new nodes connects them all.
- When a node joins the cluster (enrolled into it or moved in), leaves it
  (moved out) or is revoked, every member of the affected networks is
  updated at once, so the others add or drop that peer without waiting.
  A move updates both the old and the new cluster's network.
- A member that is offline when a node is revoked keeps the revoked peer in
  its WireGuard config until it reconnects and is updated.
- Healthy nodes are re-checked every 10 minutes, which repairs drift.
- **Re-sync now** in a node's row menu retries it at once
  (`forge clusters network resync <cluster> <node-name>`).

## Container network

Once a Linux member's WireGuard part is in sync, Forge gives that node a
Docker network for its containers. Its subnet is the node's instance block,
which every peer already routes over the mesh. A container on one node
reaches a container on another node at its own address, without a published
port and without address translation.

| Property | Value |
| --- | --- |
| Docker network name | `forge-<interface>`, for example `forge-forgea1b2c3` |
| Bridge device | `fg` + the first 8 characters of the interface, for example `fgforgea1b` |
| Subnet | The node's instance block (a /24 on a /16 network, a /28 on a /24 network) |
| MTU | `1420` |
| Gateway mode | `routed`: no masquerade, no inbound filtering for this network |
| Labels | `forge.managed=true`, `forge.fabric=<interface>` |

Forge also checks that `net.ipv4.ip_forward` is `1`. If it is not, Forge
sets it and persists it in `/etc/sysctl.d/90-forge-fabric.conf`.

Every service of a compose stack on that node joins the network in addition
to its own networks, unless it sets `network_mode` or opts out with
`x-forge.mesh: false`. See
[Compose extensions](/docs/reference/compose-extensions/#cluster-network).
The stack preview shows, per service, whether it joins (the **Mesh** row on
each service card). In the stack editor and the **Update** dialog, the
**Cluster network** checkbox writes the stack's top-level `x-forge.mesh`.
When a node's network is `ok` but a running container has no cluster
address, the stack page shows **Not on the cluster network** with
**Redeploy**.

A native service with runtime `container` joins too: after Forge creates
the container, it connects it to the network as a second network. The
default bridge and published ports stay as they are. Set `"mesh": false` in
the [service definition](/docs/reference/cli/services/#service-definition)
to keep it off, or clear **Cluster network** under **Ports** in the
service editor. The container's inspect view shows its `mesh_ip`.

The **Container network** column in the **Nodes** table on the **Network**
page shows the network name, a state badge, `Docker <version>`, and the
error when the state is `unsupported` or `failed`. The API reports the same fields:

| Field | Values |
| --- | --- |
| `network` | `ok`; `unsupported` (Docker older than 28 or missing); `failed` (the node could not create it, for example a Docker network already uses the block); `waiting for a block` (the node has no instance block yet); absent before the first try and on Windows nodes |
| `network_error` | Why the network is not `ok`, for example `docker 28.0 or newer is needed for the cluster container network (found 27.5.1)` |
| `docker_version` | The Docker version the node reported |

An `unsupported` node stays **in sync** for WireGuard and is not retried with
backoff. Forge tries again when the node reports a different Docker version,
on **Re-sync now**, and on the 10-minute repair pass. Stacks on that node
deploy without the network.

A `failed` network also leaves the WireGuard part **in sync**: the node keeps
its mesh address and other nodes keep reaching it. Forge retries the network
on **Re-sync now** and on the 10-minute repair pass.

:::note[Limits]
The container network exists on Linux members of Default clusters only.
Swarm clusters and Windows nodes get none. Runtime `service` (systemd)
services have no container to attach.
:::

## Fix an address

Open a node's or service's row menu (or right-click its row) on the
**Network** page:

- For a service, select **Fix this address**. For a node, select
  **Fix mesh address** or **Fix instance block**.
- Enter the **Address** (or **Block (CIDR)**) and save. Forge checks that it
  is inside its region and not used twice, and never moves it.
- **Release fixed address** (or **Release fixed mesh address**,
  **Release fixed block**) returns it to automatic.

To change the subnet, select **Change network**. Changing the subnet
re-assigns every address that is not fixed.

:::caution
A new subnet gives every node a new instance block. The node's container
network cannot move while containers are attached to it, so the network
step fails on those nodes until you redeploy their stacks. **Change network**
warns before you save; after you save, it lists the nodes under **Stacks on
these nodes must be redeployed to move to the new block**. The
`PUT /api/v1/clusters/<cluster-id>/fabric` response lists them:
`restarts_containers` is `true` and `redeploy_nodes` names the nodes whose
container network was up.
:::

## Internal names

With the domain `lab.forge.internal`, the service `api` in the environment
`payments` is reachable as:

- `api` from the same environment;
- `api.payments` from anywhere in the cluster;
- `api.payments.lab.forge.internal` as the full name.

Every node resolves these names through a block that Forge keeps in its
hosts file (`/etc/hosts`, or `C:\Windows\System32\drivers\etc\hosts`),
between `# BEGIN orbyn-forge <interface>` and `# END orbyn-forge <interface>`.
It lists:

- every node as `<hostname>.<domain>` at its cluster-network address;
- every service with an active placement as `<service>.<environment>.<domain>`,
  one line per node it runs on.

Forge rewrites the block on every network pass (about every 20 seconds), so
names follow deploys and moves. Lines outside the block are never touched.

### forge-dns

Each Linux member also runs [forge-dns](/docs/reference/forge-dns/) on its
mesh address, port `53`. It serves the cluster's
[local names](/docs/concepts/local-names/#forge-dns), including one name per
compose stack service (`<svc>.<stack>.<environment>.<cluster>.<suffix>`)
and per replica (`<svc>-<n>...`), plus SRV and PTR records. Forge sets it
up when the network is applied:

1. It writes `/etc/forge/forge-dns.env` with `--listen <mesh ip>:53`.
2. It enables and starts `forge-dns.service` once the node has its first
   `/var/lib/forge-dns/names.json`.
3. With systemd-resolved, it points the cluster interface at forge-dns for
   the cluster zone and its reverse zones. Without it, the node keeps using
   the hosts file.

Stack containers on the container network of a node whose facts show
`forge_dns: running` get `dns` (the node's forge-dns,
then the server host's when [`[dns] listen`](/docs/reference/configuration/#cluster-dns)
is set) and `dns_search` from their stack, environment and cluster, unless
the service sets `dns` itself. A container then reaches `api` in its own
stack by its short name.

On a node where forge-dns is not running, containers keep Docker's own
resolver, so they still resolve outside names. Redeploy the stack once
forge-dns runs to give them cluster names.

Windows members use the hosts file only.

## Remove the network

Removing the network needs `workspace.manage` on the cluster. When the
network was applied, it also needs `server.command` on every member, which
needs an MFA-verified session.

:::caution
Nodes stop reaching each other over the mesh, and services lose their
mesh addresses and internal names. Creating the network again assigns new
addresses.
:::

1. On the cluster's **Network** page, select **Remove network**.
2. Read what the dialog lists: the nodes that lose the fabric interface, any
   member that is offline now, and the number of services that lose their
   addresses.
3. Type the cluster name and select **Remove network**.
4. If asked, confirm in **Verify it's you**.

Forge removes the container network first, then takes the interface off
every online member and removes the hosts block. It also stops and disables
`forge-dns`, reverts the interface's systemd-resolved settings and deletes
`/etc/forge/forge-dns.env` and `names.json`. A container network that
still has containers attached is not removed, and its node is reported as
not cleaned. Offline members keep a dead interface; the toast names them so you
can remove it by hand. The page then shows **Set up the cluster network**
again.

From the command line, `forge clusters network remove <cluster> --yes`
does the same. The API equivalent is
`DELETE /api/v1/clusters/<cluster-id>/fabric`. Both answer
`{ "not_cleaned": [...] }` with the members they could not clean.

## Verify

- The **Status** column on the **Network** page shows **in sync** for every
  node, and `forge clusters network get <cluster>` shows `ok` in `STATE` for
  each.
- On a Linux member, `sudo wg show` lists the cluster interface with a
  recent handshake for each peer.
- From one member, `ping <other-hostname>.<domain>` answers from the other
  node's mesh address.
- The **Container network** column shows `ok` for each Linux member.
- On a Linux member with Docker 28 or newer,
  `docker network inspect forge-<interface>` shows the node's instance block
  as the subnet.
- On a Linux member, `systemctl is-active forge-dns` prints `active`, and
  `dig +short @<mesh-ip> <node>.<cluster>.<suffix>` answers with the node's
  mesh address. The node's facts show `forge_dns: running`.
- From a stack container, `getent hosts <svc>.<stack>` lists one address per
  healthy replica.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| **waiting for peers** | Some peers have no key yet | Wait: the node is updated once they report one. Bring offline peers online |
| **retrying** with an error | A step failed on the node | Read the error; Forge retries with backoff. Select **Re-sync now** after fixing the cause |
| **needs attention** | 8 failed attempts in a row | Fix the error shown; Forge keeps retrying every 15 minutes |
| Two NATed nodes cannot reach each other | No public Linux node to relay | Add a public Linux member, or set a public endpoint with a port forward |
| Nodes show a permission denial | The person who last applied the network lost access | Select **Re-apply network** yourself |
| Network `unsupported`, error `docker 28.0 or newer is needed …` | Docker is older than 28 or missing | Upgrade Docker Engine to 28 or newer. Forge retries when the node reports the new version, or select **Re-sync now** |
| Network `waiting for a block` | The node has no instance block lease | Check the plan has a free block; fix one with **Fix instance block** |
| **retrying** with `network forge-… has <n> containers on subnet <old>; redeploy stacks to move to <new>` | The subnet changed while containers use the old block | Redeploy the stacks on that node |
| The preview says `update its agent to put services on the cluster network` | The agent does not report the `mesh_network` fact | [Update the agent](/docs/guides/update-agents/), then redeploy |
| **Not on the cluster network** on a stack | The stack was deployed before the node's network was `ok` | Select **Redeploy** |
| `forge_dns: missing` | The node was installed from a package without forge-dns | Reinstall the agent package; agent self-update does not install forge-dns yet |
| `forge_dns: stopped` | The node has no `names.json` yet, or forge-dns failed | Wait for the next names pass; else read `journalctl -u forge-dns` |
| A stack service has no name | It has no port in `ports:` or `expose:`, is off the cluster network, or no replica is healthy | Add `expose:`, redeploy onto the network, or fix the health check. The replica names `<svc>-<n>` still resolve |
| The subnet is refused | Not a private IPv4 range between /16 and /24, or it overlaps | Pick another range, or keep the suggestion |

## Next steps

- [Working inside a cluster](/docs/guides/clusters/)
- [Ingress](/docs/guides/ingress/)
- API: `/clusters/{id}/fabric`, `/fabric/suggest`, `/clusters/{id}/fabric/apply`
  and `/clusters/{id}/fabric/nodes/{node}/resync` in the
  [REST API](/docs/reference/rest-api/).
