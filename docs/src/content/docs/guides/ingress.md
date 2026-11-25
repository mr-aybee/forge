---
title: Ingress
description: Run Nginx on a cluster's public edge nodes as a standalone, active-standby or active-active profile; map domains to services; use Let's Encrypt or your own certificates; let Forge keep DNS records and fail over.
sidebar:
  order: 7
---

This guide shows you how to set up a cluster's ingress: Nginx on the edge
nodes you pick, the domains it serves, their certificates and the DNS records
Forge keeps. Edges reach services over the
[cluster network](/docs/guides/cluster-network/), so services need no public
address.

## Before you begin

- **Save & apply** on the profile, and **Apply to edges** on an endpoint,
  need `proxy.manage` on each edge (**Operator**, **Admin** or
  **Platform admin**) with an MFA-verified session.
- Editing the profile also needs `workspace.manage` on the cluster
  (**Admin** or **Platform admin**).
- Custom `http {}` directives need `server.command` (MFA) on every edge.
- Install **Nginx** from the [App Store](/docs/guides/app-store/) on every edge
  node. For Let's Encrypt, install **Certbot** on every edge. For
  Forge-managed DNS, install **PowerDNS** on a node.
  The App Store installs the distribution's Nginx package. Generated site
  configs work on Nginx 1.18 and newer: HTTP/2 is turned on with
  `listen 443 ssl http2`, which Nginx 1.25.1+ accepts with a deprecation
  warning. Proxied requests carry `Host`, `X-Real-IP`, `X-Forwarded-For`,
  `X-Forwarded-Proto` and `X-Forwarded-Host`.
- Edge nodes run Linux.

Open the cluster and select **Ingress** in the sidebar. The page has five
tabs:

| Tab | What it is for |
| --- | --- |
| **Endpoints** | Domains and the service each one goes to |
| **Profile** | The layout, the edge nodes, DNS and Nginx tuning |
| **Routing** | Each request's path: domain → edges → service → node addresses |
| **Certificates** | Certificates you bought, uploaded once and picked per endpoint |
| **Health** | Edge health and the DNS records Forge publishes |

## Set up the profile

On the **Profile** tab, open the sections you need:

1. **Layout**
   - **Standalone:** one edge serves every endpoint. Point your domains at its
     public IP yourself; Forge needs no DNS.
   - **Active · standby:** the primary serves traffic. When it fails, DNS
     moves to a backup.
   - **Active · active:** every edge serves traffic, with DNS round-robin over
     the healthy edges.
2. **Edge nodes:** tick the nodes that run Nginx. Each selected edge has a
   **public IP** field: the address clients and DNS use. With active-standby,
   one edge is the primary.
3. **Traffic & DNS**
   - **Forge-managed records:** pick a PowerDNS node (install PowerDNS from
     the App Store). Forge then keeps an A/AAAA record for every endpoint
     domain. The zone is the longest matching zone on that node.
   - **Record TTL (s):** 60 seconds by default. Lower values fail over faster.
   - **Automatic failover:** for HA layouts. Forge checks each edge every 15
     seconds: the agent must be online and Nginx on the public IP must
     answer an HTTP request. Three failed checks in a row take an edge out of DNS; three good
     checks in a row bring it back. Every move is in the audit log as
     `ingress.dns.failover`.
   - **Failover:** DNS by default. Pick
     **Virtual IP (keepalived, edges on one LAN)** only when the edges share
     one LAN.

   You manage zones, nameservers and all other records on the
   [DNS page](/docs/guides/sites-and-dns/). Forge only writes the endpoint records.
4. **Nginx tuning** (collapsed by default):
   - worker connections, timeouts, body size and rate limit;
   - gzip, HSTS and whether Nginx shows its version;
   - the access log format;
   - under **Advanced**: trusted proxies and custom `http {}` directives.

   The file is checked with `nginx -t` on every edge; if the check fails, the
   previous file is put back.

5. Select **Save & apply**. Forge writes the configuration to every edge and
   publishes DNS. Saves and applies are audited. A profile without edge
   nodes cannot be applied or removed.

From the command line, save the profile with
[`forge clusters ingress set`](/docs/reference/cli/clusters/#forge-clusters-ingress-set)
and push it with
[`forge clusters ingress apply`](/docs/reference/cli/clusters/#forge-clusters-ingress-apply).
The console's **Save & apply** does both in one step; the CLI keeps them
apart:

```sh
# One edge: a standalone profile
forge clusters ingress set <cluster> --edge <node-name> --yes
# Two edges, the first one primary
forge clusters ingress set <cluster> --edge <node-name>=<public-ip> --edge <node-name>=<public-ip> --mode active_standby --yes
# Push the saved profile to the edges
forge clusters ingress apply <cluster> --yes
```

DNS, VRRP and Nginx tuning go in a JSON file; see the
[ingress profile reference](/docs/reference/cli/clusters/#ingress-profile).

Removing the profile
([`forge clusters ingress remove`](/docs/reference/cli/clusters/#forge-clusters-ingress-remove)
on the command line) takes the ingress off every edge. Each edge the removal
succeeded on loses its edge role, so you can then move that node to another
cluster. An edge that was offline or failed keeps the role until you remove
the profile again. The other profile settings stay, so when you add an edge
again it gets the same Nginx, VRRP and DNS settings.

:::caution
Custom `http {}` directives run as root on every edge. They accept the same
[directive list as endpoint custom blocks](/docs/guides/sites-and-dns/#custom-blocks),
and saving or applying a non-empty block needs `server.command` (MFA) on every
edge.
:::

## Add an endpoint

An endpoint is one or more domains plus where requests go.

1. On the **Endpoints** tab, select **New endpoint**.
2. Enter a **Name** and the **Domains** (the first is primary).
3. Set where requests go: **Fill upstreams from a service** and its
   **Port**, or type the **Active upstreams** and **Standby upstreams** as
   `host:port`, one per line. Pick **Balancing** when the service has
   several replicas: **Round robin** (default), **Least connections** or
   **IP hash** (one client always reaches the same replica).

   You can also start from the service: **Expose through ingress** on a
   service's page opens this editor with the service and its port filled in
   (see [Run on several nodes](/docs/guides/services/#run-on-several-nodes)).
4. Choose **TLS** (see [Choose certificates](#choose-certificates)).
5. Select **Save**, then open the endpoint's row menu and select
   **Apply to edges**.

Each target is one of:

- **Service:** a Forge service in this cluster, picked by environment and
  name. Forge sends traffic to every node the service runs on, at the address
  described in [Which node address the edges use](#which-node-address-the-edges-use). Replicas (active placements) are active upstreams; standby
  placements are used only when all replicas are down. Upstreams are
  re-resolved every time you apply, so they follow deploys and moves.
- **Node port:** a node and a port. Forge picks its address the same way.
- **Address:** any `host:port`.

The endpoint takes its edges from the profile.

To check an endpoint before you save it, send its body to
`POST /api/v1/sites/preview` (the same body as `POST /api/v1/sites`). The
answer shows the rendered Nginx config the edges will run (`config`: the
HTTPS variant with TLS, the HTTP variant when TLS is off), the edges and, per
target, the upstream addresses the edges would use. Nothing is stored and no node is
contacted. See the [REST API reference](/docs/reference/rest-api/#ingress-sites-and-dns).

### Which node address the edges use

For **Service** and **Node port** targets, Forge uses the node's
cluster-network (mesh) address only when both of these hold:

- the node's mesh is up in its cluster network, and
- every edge of the endpoint is up in the same cluster network.

Otherwise Forge uses the node's own address, the one the server sees it
connect from. An edge in another cluster cannot reach a mesh address, so an
edge in cluster `control` forwarding to nodes in cluster `lab` gets the
nodes' LAN addresses. One upstream list serves every edge, so a single edge
outside the network is enough for Forge to use the own addresses. This is
the same rule as [local names](/docs/concepts/local-names/). Upstreams are
re-resolved each time you apply the endpoint.

:::caution
An edge outside the targets' cluster network reaches them over the plain LAN,
not the encrypted mesh. Anyone on that network can read or change the
traffic between the edge and the backend. To keep it encrypted, put the edge
in the same cluster network as the backends, or use TLS between the edge
and the backend.
:::

When the profile publishes DNS (active-standby or
active-active with a PowerDNS node), the profile owns the records for every
endpoint and moves them off failed edges. The endpoint's DNS tab then only
shows that. Per-endpoint records are for profiles without DNS, for example a
standalone edge whose records you still want Forge to write.

## Choose certificates

Each endpoint picks one TLS option:

- **Off:** HTTP only.
- **Let's Encrypt:** free, and never left to expire. Forge checks every edge
  every 6 hours. When 5 days or fewer are left (or the certificate is
  missing), it renews it on that edge, checks the config with `nginx -t` and
  reloads Nginx. A failed renewal is retried every hour and shows as
  **renewal failing** on the endpoint. Certbot's own timer still renews
  earlier (at 30 days), so Forge's check is a safety net. **Renew certificate
  now** on the endpoint menu forces a renewal. Every renewal is audited
  (`site.certificate.renew`). Install **Certbot** on every edge. With
  several edges, each edge gets its own certificate. When Let's Encrypt
  checks an edge that does not hold the challenge, that edge asks the other
  edges over the cluster network. So issuing works whichever edge DNS points
  at.
- **Uploaded certificate:** for paid or company certificates. Upload the chain
  and the private key on the **Certificates** tab with **Upload**, then pick
  it under **Certificate** on the endpoint. Forge checks that:
  - the key belongs to the certificate;
  - the certificate has not expired;
  - it covers the endpoint's domains.

  The tab shows days left. The private key is encrypted at rest and never
  shown again. It is decrypted only into the command that sends the endpoint
  to an edge, and the edge stores it in `/etc/nginx/forge/tls` with mode
  `0600`. A certificate used by an endpoint cannot be deleted. Uploaded
  certificates cannot renew themselves, since a new one has to come from your
  CA. The endpoint turns amber 14 days before expiry so you can upload the
  replacement in time.

## Check routing

The **Routing** tab shows, for each endpoint:

- the domain;
- the edges, with their Nginx state;
- the target service and its internal name
  (`api.payments.<cluster>.forge.internal`);
- the node addresses Nginx forwards to, each with the placement's last state
  and its role (`replica` or `standby`).

An upstream whose node is offline is dimmed and marked **node offline**. An
upstream whose service is stopped, failed or not deployed is dimmed and marked
**not running**. An edge node whose agent is offline is marked the same way.
Under the domain, the endpoint shows **degraded** with the count of upstreams
up when some are down, and **down** when none is up. The view updates when a
node goes offline or comes back, and when a service starts or stops.

:::note
Upstream states come from the agent's connection and the state the node last
reported. They are not health checks: Nginx may still try a dimmed upstream
until its own failure detection skips it.
:::

The link at the bottom opens the cluster network page.

### Endpoints with explicit edges

An endpoint created with hand-picked edge nodes (`edges`, no `cluster_id`)
is not linked to any cluster. It still shows on the cluster's **Ingress**
page, in **Endpoints** and **Routing**, when one of its edges is a node of
that cluster. There it carries the badge **explicit edges · not linked to
profile**, and `GET /api/v1/clusters/{id}/routing` returns it with
`"source": "edges"`. It also stays on the workspace **Ingress** page.

To hand the endpoint over to the cluster's ingress profile:

1. On the cluster's **Ingress** page, open **Endpoints**.
2. In the endpoint's row menu, select **Link to cluster**, then confirm.
   The action needs an ingress profile on the cluster and `workspace.manage`.
3. If one of the old edges is not in the profile, select **Remove from
   edges** first; linking does not take the config off nodes that leave.
4. Select **Apply to edges**.

The endpoint then takes its edges from the profile and shows without the
badge.

## Watch traffic

Every edge logs one short line per proxied request to
`/var/log/nginx/forge-traffic.log` (time, endpoint, upstream, bytes in and
out). Each endpoint's own config writes this log, so it works on edges with
or without a cluster ingress profile, including endpoints with explicit
edges. It stays on even when the main access log is off. Its agent sums the
last minute and reports it with its metrics, so the console shows throughput
in KB/s and MB/s:

- On **Routing**: on each line, from domain to edge (client traffic) and from a
  service to each node it runs on (traffic to and from that node, summed
  over all edges). Hover a figure for requests per second and the in/out split.
- On **Health**, under **Load**: per edge (clients in / out, requests per second) and per
  node behind the edges, largest first.
- On the cluster's **Overview** and **Nodes**: *Route traffic* is the combined figure through all
  edges, and the **Traffic** column next to Memory shows what the edges send
  to each node (↓) and get back (↑).

`GET /api/v1/traffic?cluster_id=` returns the same figures. Upstreams that are
not Forge nodes (external addresses) are listed under `unmatched`.

## Verify

- **Health** shows every edge as healthy, and the DNS records Forge
  publishes.
- `curl -I https://<domain>` from outside returns your service's response
  with a valid certificate.
- **Routing** shows each endpoint's node addresses with their last state.
  Turn off one upstream node: its upstream turns dim with **node offline**,
  and the endpoint shows **degraded**.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| **New endpoint** is disabled with "Configure the ingress profile first" | The cluster has no ingress profile | Set up the profile first |
| Apply fails and the previous file is back | `nginx -t` rejected the configuration | Read the error; fix the custom directives or tuning, then apply again |
| Endpoint shows **renewal failing** | Let's Encrypt could not reach the challenge, or Certbot is missing | Point the domain at the edges, install **Certbot**, then **Renew certificate now** |
| Certificate upload is refused | The key does not match, the certificate expired, or it does not cover the domains | Upload the matching chain and key |
| An edge is taken out of DNS | Three failed health checks in a row | Check the agent and Nginx on that edge; it returns after three good checks |
| "Install PowerDNS from the App Store first" under **Forge-managed records** | No node has PowerDNS | Install **PowerDNS** on a node |
| `502` from the edge | The edge cannot reach the upstream | Check the cluster network and the service's port |
| Upstreams use the LAN address, not the mesh address | An edge is outside the targets' cluster network, or a node's mesh is not up | Expected; see [Which node address the edges use](#which-node-address-the-edges-use). Put the edges in the same cluster to use the mesh |

## Next steps

- [Sites and DNS](/docs/guides/sites-and-dns/)
- [`forge clusters ingress`](/docs/reference/cli/clusters/#forge-clusters-ingress-get)
  and [`forge sites`](/docs/reference/cli/sites-certificates/) for the same tasks
  from the command line.
- [Cluster network](/docs/guides/cluster-network/)
- [App Store](/docs/guides/app-store/) for Nginx, Certbot and PowerDNS.
