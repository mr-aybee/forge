---
title: Local names
description: How every node learns the names of the fleet's nodes and services, where the addresses come from, and the trade-offs of the .local suffix.
sidebar:
  order: 6
---

This page explains how Forge gives every node a name like `web-1.local`
that every other node can resolve, and why it works the way it does.

## What gets a name

Forge keeps one list of names per workspace and writes it to every enrolled
node, whether the node is in a cluster or not.

| What | Name in a cluster | Name outside a cluster | Addresses |
| --- | --- | --- | --- |
| A node | `<node>.<cluster>.local` | `<node>.local` | One |
| A service (a service with an active instance) | `<app>.<environment>.<cluster>.local` | `<app>.<environment>.local` | One per node it runs on |
| A compose stack service | `<svc>.<stack>.<environment>.<cluster>.local` | none | One per running container that is healthy or has no health check |
| One replica of a compose stack service | `<svc>-<n>.<stack>.<environment>.<cluster>.local` | none | The address of container number `<n>`, whatever its health |

`local` is the default suffix. See [The suffix](#the-suffix) to change it.

### Stack names

Stack names point at the containers' own addresses on the
[container network](/docs/guides/cluster-network/#container-network), not
at the node. These rules apply:

- Only a compose service with a container port in `ports:` or `expose:` gets
  a name. A service with neither has none.
- Only containers on the cluster network get a row. A container with
  `x-forge.mesh: false` or `network_mode` has no name.
- `<n>` is the Compose container number (`api-1`, `api-2`).
- The stack service name drops a container that is unhealthy or still
  starting, so callers reach only healthy replicas. The replica name keeps
  it, so you can still reach a sick container to debug it.
- The names follow containers within about 10 seconds. The agent reports
  its containers when they change, without waiting for a deploy.
- These addresses are reachable only inside the fabric, so only nodes in
  the same cluster network see these names.

Native services with runtime `container` keep the node's mesh address.
They have no per-container name yet.

### When names clash

- A node name wins over a service name.
- A replica name that equals a service or stack service name is dropped.
  The stack plan shows an Info finding such as
  ``service `api-2` clashes with replica 2 of `api` ``.
- A stack and a native service with the same name do not clash: the stack
  service is `api.orders.prod...`, the native service `orders.prod...`.

## The suffix

Every name ends in one suffix, shared by the whole fleet. Forge takes the
first of these that is set:

1. The runtime setting, which a platform admin changes in the console
   (**DNS** → **Local names** → **Local name suffix**), with
   `forge dns suffix set` or with `PUT /api/v1/names/suffix`. It is kept in
   the settings store and needs no restart.
2. [`[names] suffix`](/docs/reference/configuration/#local-names) in
   `forge.toml` (or `FORGE_NAMES_SUFFIX`).
3. `local`.

A suffix is one or more lowercase DNS labels separated by dots, at most 63
characters in all, for example `local`, `home.arpa`, `corp.internal` or
`lab.example.com`. One label is accepted only when it can never be a public
top-level domain (`local`, `internal`, `lan`, `home`, `test`,
`localdomain`, `intranet`, `corp`), so `com`, `net`, `org` and `io` are
refused. A suffix with `localhost` is refused too. The control plane and
the agent apply the same rules: a node checks every name again against the
suffix it receives.

When the suffix changes, Forge rebuilds every node's list and sends it on
the next pass. The node replaces its whole block, so the old names stop
resolving on every node at once; nothing keeps them as aliases. Forge
containers get the new names on their next sync. Update anything that
still uses the old names, such as service settings or bookmarks.

Changing the suffix needs `system.update` on the platform (the
**Platform admin** role) and an MFA-verified session when MFA is required,
because it changes every node. Each change is audited as
`names.suffix.update` with the old and new value.

## Where the label comes from

A node's label is, in this order:

1. the **DNS name** you set (**Rename node** in the console,
   `forge nodes dns-name`, or `PUT /servers/{id}/dns-name`);
2. otherwise, when the node first gets a label, its console name
   (`Web Server` → `web-server`);
3. otherwise the first part of its hostname (`web-01.example.com` →
   `web-01`).

A node gets its label once, when Forge first sees it, and keeps it. A
derived label that another node already has gets the end of the node's id
added (`web-01-3f9a2c`). Labels never move on their own:

- renaming a node in the console does not change its label (set a DNS name
  for that);
- renaming one node never changes another node's label;
- a name freed by a rename stays free until someone sets it as a DNS name.

Forge refuses a DNS name another node has now (`409`). A user who may rename
one node cannot move another node off its name.

## Where the addresses come from

```diagram
alt: Which address node A uses for node B
flow:
  rows:
    - title: same fabric?
      steps:
        - { label: node A, edge: yes }
        - { label: mesh address of B, note: "`10.64.x.y`", tone: accent }
    - title: otherwise
      steps:
        - { label: node A, edge: "" }
        - { label: "the address Forge sees B's agent connect from" }
```

- Nodes in the same [cluster network](/docs/guides/cluster-network/) use each
  other's mesh addresses.
- Every other pair uses the address the Forge server sees the node's agent
  connect from. A node behind NAT shows its NAT address.
- Forge never takes an address or a name from what a node says about
  itself. Hostnames only feed a derived label after it is reduced to
  `a-z`, `0-9` and `-`; ports come from the stored service specs.

## How a node keeps them

A node resolves names in two ways:

| Path | Nodes | Serves |
| --- | --- | --- |
| Hosts file | Every node, Linux and Windows | A names (one line per address) |
| forge-dns | Linux nodes in a cluster network | A, SRV and PTR records for the node's cluster |

### Hosts file

The node's agent replaces one marked block in its hosts file and leaves
every other line alone:

```text
127.0.0.1   localhost
# BEGIN orbyn-forge names
# Names on the Forge network. Managed by Orbyn Forge; edits are overwritten.
192.0.2.11   lab-b.local
192.0.2.12   lab-c.local
# END orbyn-forge names
```

On Linux the root helper writes `/etc/hosts`; on Windows the agent writes
`C:\Windows\System32\drivers\etc\hosts`. The node also writes the block into
the hosts file of each of its Forge containers, so a container on that node
resolves the same names.

The file is rewritten in place, not replaced. Bind mounts of `/etc/hosts`
and Docker's per-container hosts files keep seeing the new content. Owner,
mode and SELinux context stay as they were.

:::caution
The write is not atomic. If the node crashes between emptying the file and
writing it, the hosts file can be left empty or partial. The next sync
writes Forge's block again, but Forge cannot rebuild your other lines, so
keep a copy of a hand-made hosts file. Files larger than 16 MiB (4 MiB for a
container) are refused, never cut.
:::

Forge sends the list:

- within 30 seconds of a change (about 2 seconds after a rename or
  enrollment; changes that arrive together, such as many nodes enrolling,
  make one rebuild);
- at most once every 10 seconds per node while its list keeps changing,
  so a large fleet enrolling does not resend every list on every change;
- when a node reconnects;
- every 10 minutes, which repairs hand edits and containers that were
  restarted since.

Nodes that see the same names share one list: Forge builds one list per
fabric, plus one for each node whose own address is a loopback address, and
reads each table once per rebuild. The cost of a rebuild grows with the
number of names, not with the number of nodes times the number of names.

The node checks every entry again before it writes: each name must be made
of DNS labels and end in a safe suffix, and each address must parse.

### forge-dns

On a Linux node in a cluster network, the agent also runs
[forge-dns](/docs/reference/forge-dns/), a small DNS server that listens on
the node's mesh address, port `53`. The helper writes the cluster's records
to `/var/lib/forge-agent/names.json`, and forge-dns reloads it within about
2 seconds. Records have a TTL of 5 seconds.

forge-dns answers more than a hosts file can:

- **A** records, rotated on each query, so clients spread over replicas.
- **SRV** records `_<service>._<proto>.<name>` with the service's first
  container port. These exist only in forge-dns, never in the hosts file.
- **PTR** records for every address inside the fabric subnet. A replica or
  node name is preferred as the target, so a reverse lookup names one
  container.

Names outside the cluster zone are forwarded to the node's usual resolvers.

**Containers** on the cluster network get these DNS settings, unless the
compose service sets `dns` itself:

- `dns`: the node's mesh address first, then the server host's forge-dns
  when [`[dns] listen`](/docs/reference/configuration/#cluster-dns) is set.
  Docker's embedded resolver tries them in order.
- `dns_search`: `<stack>.<environment>.<cluster>.<suffix>`,
  `<environment>.<cluster>.<suffix>`, `<cluster>.<suffix>`. A container
  reaches another service of its stack as `api`, and one in another stack as
  `api.orders`.

**The node itself** uses forge-dns only with systemd-resolved: the agent
runs `resolvectl dns <iface> <mesh ip>` and
`resolvectl domain <iface> ~<zone> ~<reverse zones>`. This is scoped to the
WireGuard interface, so the node's other resolvers stay untouched. Without
systemd-resolved, the node changes nothing and the hosts file still serves
its names.

The **server host tier** is optional. With `[dns] listen` set, the server
writes `<data_dir>/names.json` with every cluster of every workspace, and
its own forge-dns answers when a node's forge-dns is down.

A node's facts report `forge_dns` as `running`, `stopped` or `missing`.
Windows nodes and nodes outside a cluster network use the hosts file only.

:::note
Agent self-update does not update the forge-dns binary yet. Reinstall with
the package to get a newer forge-dns.
:::

## Ports

A hosts file holds names and addresses, not ports. The ports a service
publishes on its node are listed in the console's **DNS** page (**Local
names**), in `forge dns list` and in `GET /api/v1/names`.

## `.local` and mDNS

`.local` is also the domain of multicast DNS (mDNS, RFC 6762), which Avahi
on Linux and Bonjour on macOS use. On Debian, Ubuntu and RHEL-likes the
hosts file wins while `/etc/nsswitch.conf` lists `files` before
`mdns4_minimal [NOTFOUND=return]`, which is the default.

:::caution
Programs that ask mDNS directly (Avahi tools, some browsers, macOS clients
on the LAN) do not read the hosts file. Another machine on the LAN can
answer `*.local` over mDNS, and Avahi on a node may announce
`<hostname>.local` itself. If that matters on your network, change the
[suffix](#the-suffix) to `home.arpa` (reserved for home networks, RFC 8375),
`internal`, `lan` or a domain you own.
:::

## Existing cluster networks

Cluster networks made before local names keep their own domain (for
example `lab.forge.internal`) and its block, so those names keep working.
New cluster networks default to `<cluster>.local`.

## Related

- [Rename a node and reach services by name](/docs/guides/local-names/)
- [`[names]` settings](/docs/reference/configuration/#local-names)
- [Change the local name suffix](/docs/guides/local-names/#change-the-suffix)
- [`forge dns list` and `forge dns suffix`](/docs/reference/cli/dns/) and
  [`forge nodes dns-name`](/docs/reference/cli/nodes/#forge-nodes-dns-name)
- [REST API: local names](/docs/reference/rest-api/#local-names)
