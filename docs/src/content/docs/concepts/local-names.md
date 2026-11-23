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

| What | Name in a cluster | Name outside a cluster |
| --- | --- | --- |
| A node | `<node>.<cluster>.local` | `<node>.local` |
| A service (a service with an active instance) | `<app>.<environment>.<cluster>.local` | `<app>.<environment>.local` |

A service on three nodes has three lines with the same name, one per node.
`local` is the default suffix. See [The suffix](#the-suffix) to change it.

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
