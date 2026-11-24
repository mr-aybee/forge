---
title: Network ports
description: Every port Forge listens on, on the control plane and on nodes, with its direction, binding, protocol and the setting that changes it.
sidebar:
  order: 10
---

This page lists the ports Forge opens, who connects to them and how to
change them. Use it to write firewall rules between nodes and the control
plane.

## Control plane

| Port | Protocol | Listener | Who connects | Default bind | Setting |
| --- | --- | --- | --- | --- | --- |
| `3880/tcp` | HTTP | API, console, `/mcp`, installer downloads, off-fleet log ingest (`POST /api/events/raw`) | Browsers, the `forge` CLI, API clients, nodes during install | `127.0.0.1:3880` (the installers write `0.0.0.0:3880`) | `api_listen` |
| `3843/tcp` | HTTP/2 over mTLS | Agent hub: enrollment, heartbeats, commands, port tunnels | Every node's agent | `0.0.0.0:3843` | `agent_listen` |
| `3832/tcp` | HTTP/2 over mTLS | Analytics worker spool uploads (`POST /agent/v1/spool`) | Every node's agent, while analytics is on | `0.0.0.0:3832` | `[analytics] worker_agent_listen` |
| `2022/tcp` | SSH (SFTP) | SFTP gateway for temporary access | SFTP clients | `0.0.0.0:2022`, only when `[sftp] enabled` | `[sftp] listen` |
| `18123/tcp` | HTTP | Embedded analytics engine | The analytics worker only | `127.0.0.1` | `[analytics] embedded_port` |

:::note[Upgrading an older install]
Updates do not rewrite `forge.toml`, and an older install keeps its ports:
`api_listen` on `8080` and `agent_listen` on `7443`, which the installer wrote
into `forge.toml`, and the analytics upload port on `55132`, which is still the
built-in default when `forge.toml` names none. Only new installs get `3880`,
`3843` and `3832`, which the installer writes into `forge.toml`.
:::

The analytics worker's control channel to `forge-server` is a Unix socket
on Linux and a loopback TCP port on Windows; it needs no firewall rule.

:::caution
Nodes must reach both `3843/tcp` and `3832/tcp` on the control plane.
With `3832/tcp` blocked, nodes stay online and keep working, but their
history, top processes, container output and application logs stay in
the node's spool until the port opens. A spool that fills drops its
oldest files.
:::

`3832/tcp` accepts only clients with a certificate from Forge's internal
CA whose node is enrolled and active, and only the node's current
certificate: one replaced at renewal is refused. The worker gets its own
server certificate (valid 90 days) from `forge-server` at each start, and a
new one at two thirds of its life while it runs, without a restart. Its
names are `agent_hostnames`, so the host in `[analytics] worker_agent_url`
must be one of them.

## Nodes

Agents dial out. A node needs no inbound port from the control plane,
unless you enable its Docker Engine API endpoint.

| Port | Protocol | Listener | Who connects | Bind | Platforms |
| --- | --- | --- | --- | --- | --- |
| `5341/tcp` | HTTP | Local log listener, Seq-compatible `POST /api/events/raw` | Apps on the node (host processes and containers) | `127.0.0.1` and the Docker bridge gateway (a private IPv4 such as `172.17.0.1`). Never a wildcard address | Linux; Windows on loopback only |
| `2376/tcp` | HTTPS (mutual TLS) | Docker Engine API, opt-in per node | The control plane | The node address you choose when you enable it | Linux |

`5341/tcp` is not reachable from the network, so no firewall rule is
needed. Containers reach it as `forge-node`: Forge adds
`--add-host forge-node:host-gateway` to the containers and compose stack
services it deploys. See
[Send and search service logs](/docs/guides/application-logs/).

The agent reads the bridge gateway from the Docker engine. When that
address is not on the host (a node that itself runs in a container and
uses the host's Docker socket), the agent logs one `INFO` line, serves
`127.0.0.1` only, and tries again only when the engine reports another
gateway. Containers on such a node cannot reach `forge-node:5341`. Any
other bind failure, such as a port in use, is logged once as a warning and
retried after 1, 2, 4 and 8 minutes, then every 15 minutes.

## Firewall rules

| From | To | Port | Needed for |
| --- | --- | --- | --- |
| Operators and API clients | Control plane | `3880/tcp` (or your TLS proxy's port) | Console, API, CLI |
| Nodes | Control plane | `3880/tcp` (or `node_public_url`'s port) | Installer and agent downloads |
| Nodes | Control plane | `3843/tcp` | Agent link |
| Nodes | Control plane | `3832/tcp` | Analytics uploads |
| Off-fleet apps | Control plane | `3880/tcp` (or your TLS proxy's port) | Log ingest from machines that are not nodes |

What the installers open: see
[Server installer](/docs/reference/cli/server-installer/#what-it-does).
The installers open the ports of `api_listen`, `agent_listen` and
`[analytics] worker_agent_listen` from `forge.toml` (by default `3880/tcp`,
`3843/tcp` and `3832/tcp`) in the host firewall: firewalld or ufw on Linux,
Windows Firewall on Windows. Open them in any network firewall between the nodes and the
server yourself.

## Related

- [Configuration](/docs/reference/configuration/)
- [Agent protocol](/docs/reference/agent-protocol/)
- [Docker Engine API](/docs/concepts/docker-engine-api/)
