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

:::note[Limits]
Forge connects nodes with an encrypted WireGuard mesh only; there is no
unencrypted routed mode. Service IPs are reserved in the address plan
but not configured on nodes, and there is no firewall between environments and
clusters and no internal DNS resolver yet. Names resolve through the hosts
file (see [Internal names](#internal-names)).
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
the `host:port` the router forwards to the node's WireGuard port.

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
- **Re-sync now** in a node's row menu retries it at once.

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

Forge takes the interface off every online member and removes the hosts
block. Offline members keep a dead interface; the toast names them so you
can remove it by hand. The page then shows **Set up the cluster network**
again.

The API equivalent is `DELETE /api/v1/clusters/<cluster-id>/fabric`. It
answers `{ "not_cleaned": [...] }` with the members it could not clean.

## Verify

- The **Status** column on the **Network** page shows **in sync** for every
  node.
- On a Linux member, `sudo wg show` lists the cluster interface with a
  recent handshake for each peer.
- From one member, `ping <other-hostname>.<domain>` answers from the other
  node's mesh address.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| **waiting for peers** | Some peers have no key yet | Wait: the node is updated once they report one. Bring offline peers online |
| **retrying** with an error | A step failed on the node | Read the error; Forge retries with backoff. Select **Re-sync now** after fixing the cause |
| **needs attention** | 8 failed attempts in a row | Fix the error shown; Forge keeps retrying every 15 minutes |
| Two NATed nodes cannot reach each other | No public Linux node to relay | Add a public Linux member, or set a public endpoint with a port forward |
| Nodes show a permission denial | The person who last applied the network lost access | Select **Re-apply network** yourself |
| The subnet is refused | Not a private IPv4 range between /16 and /24, or it overlaps | Pick another range, or keep the suggestion |

## Next steps

- [Working inside a cluster](/docs/guides/clusters/)
- [Ingress](/docs/guides/ingress/)
- API: `/clusters/{id}/fabric`, `/fabric/suggest`, `/clusters/{id}/fabric/apply`
  and `/clusters/{id}/fabric/nodes/{node}/resync` in the
  [REST API](/docs/reference/rest-api/).
