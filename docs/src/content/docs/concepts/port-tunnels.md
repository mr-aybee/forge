---
title: Port tunnels
description: How forge tunnel forwards a local port to a service on a node through the control plane, who may open one, and what keeps it safe.
sidebar:
  order: 8
---

This page explains how a port tunnel reaches a service on a node, such as
PostgreSQL on port 5432 or a web app on 8080, without opening that port to
the network. Tunnels are the only remote path into a node besides the
browser terminal: everything goes through the API port.

## The path of a connection

```diagram
alt: Port tunnel path
flow:
  steps:
    - { label: psql, edge: "" }
    - { label: "`127.0.0.1:15432`", note: forge CLI, edge: WebSocket }
    - { label: Forge server, tone: accent, edge: "HTTP/2 (dialled by the node)", dir: back }
    - { label: agent, edge: "" }
    - { label: "`127.0.0.1:5432` on the node" }
```

1. `forge tunnel open` names a **target**. Forge resolves it in the service
   catalogue to one or more (node, port) pairs, checks that you may reach
   each, and registers a tunnel row per pair. The CLI listens on a local
   port and keeps a control connection open for as long as the tunnel lives.
2. Each connection your program makes to the local port gets its own
   WebSocket to the Forge server.
3. The server sends the node's agent a signed instruction with the port and
   the address to dial. The agent connects to it on the node and opens a
   data stream back to the server.
4. The server joins the two halves. Bytes pass through unchanged.

The node dials out, so a tunnel works behind NAT and needs no inbound port
on the node. The service itself can stay bound to `127.0.0.1`.

## Targets and the service catalogue

A tunnel points at a **service**, not a bare port. Forge builds a read-only
catalogue of what listens where from data it already has:

| Target | Ports | Nodes | The agent dials |
| --- | --- | --- | --- |
| Service (native) | its `listen_port` | where it is placed | `127.0.0.1` |
| Service (container) | its published host ports | where it is placed | the bound address, else `127.0.0.1` |
| Stack service | the compose file's published ports | the node of that part of the stack | the published host address: loopback or the node's cluster-network address |
| Database package | the package's ports (and a `port` parameter when set) | where it is installed | `127.0.0.1` |
| SSH | the node's SSH port | the node | `127.0.0.1` |
| Raw port (administrators) | what you name | the node | `127.0.0.1` |

Each service page in the console shows this list as its **Ports** block with
the `forge tunnel open` line for each. One tunnel row covers one (target,
node, remote port); `--all` opens several at once. The agent accepts only
loopback, the node's own addresses and its cluster-network address as a
dial address, never another machine.

## Who may open one

Two paths open the door; either is enough:

- **A role on the environment.** `network.tunnel` on the service's environment:
  **Developer**, **Operator**, **Workspace admin** and **Platform admin**
  hold it. A database package carries the environment of its installation.
  SSH targets reuse `terminal.open` on the node.
- **An explicit grant.** An administrator (`access.manage`) grants one user
  one target, optionally one node and specific ports, with its own expiry.
  Grants are for people without the role: a contractor who needs one
  database for a day. Revoking a grant closes its tunnels within 30
  seconds.

Raw ports on a node (`--port` without a service) need `network.tunnel.any`,
held by **Workspace admin** and **Platform admin** only, and are audited as
raw. Every path needs an MFA-verified session; AI connectors cannot open
tunnels.

## Lifetime

A tunnel lives for the shortest of: what you asked for with `--for`
(8 hours by default), the grant it runs under, and the server's
`[tunnels] max_lifetime_hours` (168, 7 days). `--for never` works only when
the server sets `[tunnels] allow_never_expiry = true` and you are an
administrator. A tunnel also closes after 15 minutes without traffic.

## Remembered local ports

A free local port is a property of your computer, so the CLI remembers it
in its context file, keyed by target, node and remote port. The next open of
the same target gets the same local port; `-l` overrides it once and becomes
the new memory; `forge tunnel ports` lists and forgets mappings. The server
stores nothing about local ports.

## What keeps it safe

- **Service, not port.** You cannot reach a port the catalogue does not list
  for that service, so a database on a node does not become reachable
  because a web app on the same node is.
- **Local addresses only.** The agent dials loopback, the node's own
  addresses or its cluster-network address. A tunnel cannot reach other
  machines, cloud metadata addresses or anything else on the node's
  network.
- **Limits.** By default 8 tunnels per user, 32 connections per node and
  256 per server. See [`[tunnels]`](/docs/reference/configuration/#port-tunnels).
- **Revocation.** Every 15 seconds Forge checks that your sign-in or API key
  still exists, that you still hold the role or the grant, and that the
  tunnel has not expired. Signing out everywhere, deleting the key, losing
  the role, a revoked grant or an administrator closing the tunnel ends
  every connection within 30 seconds. Refreshing your sign-in token does
  not: the CLI hands Forge the new token, so a tunnel can outlive the
  session that opened it, up to its lifetime.
- **Persistence.** Tunnels are rows, not only memory: after a server restart
  the open ones are recorded as closed with the reason `server_restart`,
  and **Administration → Tunnels → History** shows every closed tunnel with
  its reason, bytes and duration.
- **Audit.** Opening, closing (with bytes and duration), revoking, and
  creating or revoking grants are audited. The data is never recorded.

:::caution
A tunnel reaches services that trust their own machine. PostgreSQL with
`trust` authentication for local connections, Redis without a password, or
an admin UI on `127.0.0.1` are open to **anyone who may tunnel to that
service**. Give roles on an environment and grants with that in mind, and
prefer short grants.
:::

:::caution
`--bind` other than `127.0.0.1` lets other machines use your tunnel. The
CLI refuses it unless you add `--allow-remote-bind` and prints a warning.
The bind address in the audit log is what the CLI reported: Forge cannot
verify where your computer actually listens.
:::

## Throughput

The bytes travel raw over a stream with flow control on every leg, so a
slow side slows the fast one instead of filling memory. On one machine,
4 MiB crossed the whole path in each direction in about 0.2 seconds. Over a
network, expect the slower of your link to the Forge server and the node's
link to it.

## Related

- [Open a port tunnel](/docs/guides/port-tunnels/)
- [`forge tunnel`](/docs/reference/cli/tunnel/)
- [REST API: port tunnels](/docs/reference/rest-api/#port-tunnels)
