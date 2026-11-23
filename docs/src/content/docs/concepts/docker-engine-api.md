---
title: Docker Engine API
description: How Forge reaches a node's dockerd directly over mutual TLS, how each call is authorized and audited, and what the open port costs you.
sidebar:
  order: 15
---

Forge can give you a node's full Docker Engine API, the same API the
`docker` CLI, Portainer or Swarm use, through the control plane. This page
explains the path a call takes, the certificates that protect it, how Forge
decides who may make which call, and the trade-off against the agent path.
To turn it on, see [Use a node's Docker Engine API](/docs/guides/docker-engine-api/).

## Two paths to Docker

Forge already manages containers through the node's agent: the
**Containers** page, compose stacks, container shells and
`POST /servers/{id}/containers` send signed commands over the agent's
outbound link, and the root helper talks to Docker on its local socket.
That path stays as it is, and it needs no open port on the node.

The Docker Engine API endpoint is a second, opt-in path for the whole API.
When you enable it on a node, dockerd also listens on TCP (port `2376` by
default) with mutual TLS, and the control plane dials it directly:

```diagram
alt: Docker Engine API proxy path
flow:
  direction: column
  steps:
    - { label: forge CLI / curl / console, edge: "HTTPS `/api/v1/servers/{id}/docker/engine/{path}`" }
    - { label: Control plane, note: "catalog match → body guard → Cedar → audit → forward", tone: accent, edge: "TCP 2376, mutual TLS (node's own Docker CA)" }
    - { label: Node, note: "`dockerd -H fd:// -H tcp://<ip>:2376 --tlsverify`" }
```

Callers never talk to dockerd themselves. They call the Forge API, which
checks the call and then forwards it.

## Certificates

Each node with the endpoint has its **own Docker CA**, created when you
enable it.

- **The CA key** is sealed with the server's master key. The associated
  data is `docker-ca:<server_id>`, so a sealed key copied to another node's
  row does not open.
- **dockerd's key** is made on the node by the root helper and never leaves
  it. The node sends a certificate signing request; the node's CA signs it.
- **dockerd's certificate** carries the DNS name
  `<server_id>.docker.forge.internal` and the listen and dial addresses as
  IP SANs. It is valid for 825 days; enabling the endpoint again issues a
  new one. The control plane checks this DNS name whatever address it
  dials, and trusts only the node's own CA: never the system roots, the
  agent CA, or another node's Docker CA.
- **The control plane's client certificates** (`CN=forge-server`) come
  from the same CA, are valid for 24 hours, live only in memory and are
  renewed after two thirds of that time. dockerd runs with `--tlsverify`,
  so it refuses any client its CA did not sign.
- **Disabling** the endpoint deletes the CA with the row. No certificate it
  signed is trusted again. Unless you force it, the node also deletes its key and certificates.

A certificate taken from one node opens nothing on another node.

## What changes on the node

Enabling is a `docker.endpoint.enable` job with four steps:

| Step | What happens |
| --- | --- |
| `prepare` | The root helper makes dockerd's key in `/etc/docker/forge-tls` and returns a CSR, the node's addresses, Docker's version, whether `live-restore` is on and how many containers run |
| `sign` | A new CA for the node signs dockerd's certificate. The endpoint is saved as `pending` |
| `apply` | The helper writes the CA and the certificate, adds a systemd drop-in and restarts dockerd. If dockerd does not come back within a minute, it puts back what was there and restarts dockerd again |
| `verify` | The control plane calls `/version` over mutual TLS for up to 45 seconds. The endpoint becomes `enabled`, or `failed` with the reason |

On the node, Forge writes:

| Path | Contents |
| --- | --- |
| `/etc/docker/forge-tls/` | Directory, mode `0700`, owned by root |
| `/etc/docker/forge-tls/key.pem` | dockerd's private key, mode `0600` |
| `/etc/docker/forge-tls/ca.pem` | The node's Docker CA certificate |
| `/etc/docker/forge-tls/cert.pem` | dockerd's certificate |
| `/etc/docker/forge-tls/execstart` | dockerd's command line before Forge changed it |
| `/etc/systemd/system/docker.service.d/forge-tls.conf` | The drop-in: dockerd's own command line plus `-H tcp://<listen>:<port> --tlsverify --tlscacert=… --tlscert=… --tlskey=…` |

The drop-in repeats dockerd's existing command line, so the local socket
and every other flag stay as they are. If dockerd already listens on TCP
outside Forge, the `apply` step refuses. Disabling (`docker.endpoint.disable`,
steps `remove` and `forget`) deletes the drop-in, restarts dockerd and
removes `/etc/docker/forge-tls`.

Restarting dockerd stops running containers unless `live-restore` is on in
`/etc/docker/daemon.json`. Containers with a restart policy start again.
Forge refuses to enable the endpoint in that case unless you pass
`acknowledge_restart`.

Every step can run twice after a crash: the node keeps its key, the signed
certificate is kept in the job state, applying the same files changes
nothing, and removing twice finds nothing to remove.

:::note
The endpoint needs a Linux node with systemd and Docker. Windows nodes are
not supported. The agent offers the `docker_tls` command kind after an
update; agents that predate it cannot run the job.
:::

## Deny by default

Forge forwards only the calls in its [catalog](/docs/reference/docker-api-catalog/):
108 Engine API endpoints. Any other method and path is refused with `404`
`docker_unknown_endpoint`. Paths with `..`, empty segments or encoded
separators are refused with `400`. Neither reaches the node.

Each catalog entry has a **class**, and each class maps to a Forge action
you already know:

| Class | Forge action | MFA | Examples |
| --- | --- | --- | --- |
| `read` | `server.manage` | No | `GET /containers/json`, logs, stats, events, inspect |
| `manage` | `container.manage` | No | Create, start, stop or remove containers; pull, tag or remove images; networks; volumes; prune |
| `exec` | `container.exec` | No | Exec, attach, archive download and upload, container export |
| `admin` | `server.command` | Yes | Swarm, plugins, build, image load, BuildKit sessions, registry login, swarm services, secrets and configs |

No new Cedar actions exist for this feature. Roles that may already manage
containers through the agent path may make the same kind of call here.

## The body guard

Some settings in a request body hand a container the host. Forge reads the
JSON body of `container.create`, `exec.create` and `volume.create` (at most
1 MiB) and raises the call to the `admin` class when it finds:

- `HostConfig.Privileged`, `CapAdd`, `Binds`, `Devices` or
  `DeviceCgroupRules`;
- a mount of type `bind`, `npipe` or `cluster`;
- the host's PID, IPC, UTS, user, cgroup or network namespace (`*Mode:
  "host"`);
- a weakened security profile in `SecurityOpt` (`unconfined`,
  `label=disable`, `seccomp=…`);
- a privileged exec;
- a `local` volume with a `device` or a `bind` option.

So an **Operator** can create an ordinary container without MFA, but a
privileged one needs `server.command` and a verified second factor. The full
list is in [Host access](/docs/reference/docker-api-catalog/#host-access).

## Secrets in container inspect

A container's `Config.Env` often holds passwords. When a caller without
`container.inspect_env` inspects a container (`GET /containers/{id}/json`),
Forge replaces every value with `<redacted>` (`KEY=<redacted>`) and adds the
response header `x-forge-redacted: Config.Env`. Callers who hold
`container.inspect_env` get the values as dockerd returns them.

## Audit

Every call outside the `read` class is audited as `docker.api` on the node,
with these details:

| Key | Value |
| --- | --- |
| `op` | The catalog op, such as `container.create` |
| `method`, `path` | The call, without the version prefix |
| `query` | The query **keys** only, never their values |
| `status` | dockerd's HTTP status, or `0` when it could not be reached |
| `class` | The effective class, after the body guard |
| `host_access` | The host-reaching settings found, if any |

Request and response bodies are never recorded. Denials are audited too.
Enabling and disabling are audited as `docker.endpoint.enable` and
`docker.endpoint.disable`.

## Streams and upgraded connections

Responses stream as dockerd sends them: logs with `follow`, `/events`, pull
progress and tar archives. Request bodies stream too, up to 8 GiB (image
loads, builds, archive uploads). Attach, exec start and BuildKit sessions
upgrade the connection to a raw duplex stream; the control plane joins the
two connections and relays bytes until either side closes. Only
allow-listed headers pass in either direction (see the
[REST API](/docs/reference/rest-api/#docker-engine-api)).

## The trade-off

The agent path needs no inbound port: the agent dials out. The Engine API
endpoint opens a port on the node that is **root-equivalent**: anyone who
can call it can start a privileged container. It is protected by mutual TLS
with a CA that exists only for that node, whose key is sealed on the control
plane, and whose client certificates live for a day.

:::caution
Whoever holds the master key and the database can open every node's Docker
CA, and so control dockerd on every node with the endpoint. Keep the port
reachable only from the control plane: pass `listen` with a private address,
and allow TCP `2376` in the node's firewall from the control plane's
address only. Enable the endpoint only on nodes that need it.
:::

Forge's own authorization, audit and body guard apply only to calls that go
through the control plane. dockerd itself checks nothing beyond the client
certificate.

## Related pages

- [Use a node's Docker Engine API](/docs/guides/docker-engine-api/)
- [Docker Engine API catalog](/docs/reference/docker-api-catalog/)
- [`forge docker`](/docs/reference/cli/docker/)
- [Authorization](/docs/concepts/authorization/)
