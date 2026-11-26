---
title: Compose extensions and policy
description: The x-forge keys Forge reads in a compose file, the placement syntax, and every setting the stack policy allows, flags or refuses.
sidebar:
  order: 8
---

Forge runs standard Docker Compose files. This page lists the extension keys
it reads, how it places services, and the policy every stack is checked
against. For the model behind it, see [Compose stacks](/docs/concepts/compose-stacks/).

## `x-forge` on a service

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `x-forge.node` | string | the node the stack was created from | Hostname (or display name) of the node that runs the service |
| `x-forge.cluster` | string | none | Name of a cluster: the service runs on every node of it. Cannot be combined with `node` |
| `x-forge.public` | list of ports, or `true` | none | Published ports that bind to all addresses (`0.0.0.0`). `true` makes every port of the service public |
| `x-forge.mesh` | boolean | `true` | `false` keeps the service off the [cluster network](#cluster-network) |

Forge removes `x-forge` from the file it writes on the node, except `public`
and `mesh`, which the node's helper reads.

## `x-forge` at the top level

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `x-forge.mesh` | boolean | `true` | `false` keeps every service of the stack off the cluster network |

```yaml
x-forge:
  mesh: false        # the whole stack stays off the cluster network
services:
  api:
    image: registry.example.com/api:1.4
  worker:
    image: registry.example.com/worker:1.4
    x-forge:
      mesh: false    # or opt out one service
```

## Cluster network

When the node's [container network](/docs/guides/cluster-network/#container-network)
is up, the node's helper attaches every service to it in addition to the
service's own networks, under the network key `forge-mesh`:

| Service as written | On the node |
| --- | --- |
| no `networks` | `networks: [default, forge-mesh]` |
| `networks` as a list | `forge-mesh` appended |
| `networks` as a map | key `forge-mesh: {}` added |
| any `network_mode` | unchanged |
| `x-forge.mesh: false`, on the service or the top level | unchanged |

When at least one service was attached, the top level gains
`networks.forge-mesh: {external: true, name: forge-<interface>}`.

The helper adds the network only when all of these hold; otherwise the
service runs as before:

- the node runs Linux in a Default cluster whose network is **in sync** on
  that node, with the container network `ok`;
- the node's agent reports the `mesh_network` fact;
- the stack's top-level `x-forge.mesh` is not `false`.

The preview shows a `mesh` value per service (`true`, `false`,
`"off (x-forge.mesh)"` or `"off (network_mode)"`) and adds an Info finding
that says why a node's services do not join:

| Finding | Cause |
| --- | --- |
| `services are reachable from other nodes only through published ports; create the cluster network to reach them by name` | The node is in no cluster network |
| `<node>: cluster network not applied yet; services will join it on the next deploy after it is in sync` | The node's mesh or container network is not up (including Docker older than 28) |
| `<node>: update its agent to put services on the cluster network` | The agent does not report the `mesh_network` fact |

### DNS on the cluster network

The helper also sets DNS on every service it attached to the cluster
network, unless the service sets `dns` itself:

| Key | Value |
| --- | --- |
| `dns` | The node's mesh address, then each IP of the server's [`[dns] listen`](/docs/reference/configuration/#cluster-dns) |
| `dns_search` | `<stack>.<environment>.<zone>`, `<environment>.<zone>`, `<zone>`, where `<zone>` is `<cluster>.<suffix>`. Not set when the service sets its own `dns_search` |

The node derives `dns_search` from the zone it was sent with its names.
Both resolvers run [forge-dns](/docs/reference/forge-dns/). See
[Local names](/docs/concepts/local-names/#stack-names) for the names a stack
gets.

## Placement

| Syntax | Meaning |
| --- | --- |
| `deploy.placement.constraints: ["node.hostname == lab-c"]` | Run on `lab-c` (Swarm style). Only `node.hostname ==` is supported |
| `x-forge: { node: lab-c }` | Run on `lab-c` |
| `x-forge: { cluster: lab }` | Run on every node of cluster `lab` |
| none | Run on the node the stack was created from |

A service may not `depends_on` a service placed on another node.

## Labels

| Label | Values | Description |
| --- | --- | --- |
| `forge.expose` | `http`, `https` | Marks the service for **Expose via Nginx** in the console |
| `forge.stack` | set by Forge | `<environment>/<stack>`, added to every service |

## Ports

Every published port binds to an internal address unless it is public:

| Port as written | On the node |
| --- | --- |
| `"8080:80"` | `<internal>:8080:80/tcp` |
| `"0.0.0.0:8080:80"` (not public) | `<internal>:8080:80/tcp`, with a note in the preview |
| `"127.0.0.1:8080:80"` | unchanged |
| `"8080:80"` with `x-forge.public: [8080]` | `0.0.0.0:8080:80/tcp` |
| `{target: 80, published: 8080}` | `host_ip` set to `<internal>` |

`<internal>` is the node's cluster network address when its mesh is up,
otherwise `127.0.0.1`.

## Policy

### Allowed

Images; named volumes; `tmpfs`; bind mounts inside the stack folder or an
approved, writable [file root](/docs/guides/approve-file-roots/) on that node;
ports; environment; `env_file` inside the stack folder; healthchecks;
resource limits; networks the stack defines; `depends_on`; labels.

External networks follow these rules:

- The network key `forge-mesh` is reserved. A stack that defines it is
  refused: ``the network key `forge-mesh` is reserved``.
- An `external` network whose Docker name starts with `forge-forge` (a
  cluster container network) is allowed only when it is the node's own
  cluster network. Otherwise the node refuses the part when it deploys: ``network `<key>` uses
  the cluster network `<name>`, which this node does not offer``.
- Other external networks keep the rules below.

### Needs `stack.privileged` (administrators, MFA)

| Setting | Why |
| --- | --- |
| `privileged: true` | Full control of the node |
| `network_mode: host`, a network with `driver: host`, or a network whose Docker name is `host` (`name: host`, or an `external` network keyed or named `host`) | The node's network |
| `pid`, `ipc`, `uts`, `userns_mode`, `cgroup`, `network_mode`: `host`, `container:…`, or `service:…` naming a service that is not in the stack (on that node) | Shares a host or another container's environment |
| `post_start` or `pre_stop` hooks with `privileged: true` | Runs a command with full control of the node |
| `use_api_socket: true` | Controls Docker on the node |
| `cap_add`, `devices`, `device_cgroup_rules`, `gpus`, `deploy.resources.reservations.devices` | Kernel capabilities and host devices |
| `sysctls`, `cgroup_parent`, `runtime` | Kernel parameters, cgroups, another runtime |
| `security_opt` with `seccomp:unconfined`, `apparmor:unconfined`, `label:disable` or `no-new-privileges:false` | Turns off a protection |
| `volumes_from: container:…` | Reads a container outside the stack |
| A bind of `/var/run/docker.sock` or `/run/containerd/containerd.sock` | Controls Docker on the node |
| A bind (or a volume with `driver_opts.device`, a config or secret `file`) outside the stack folder and the approved roots, or on `/`, `/etc`, `/root`, `/boot`, `/proc`, `/sys`, `/dev`, `/run`, `/usr`, `/var/lib/docker`, `/etc/forge`, `/var/lib/forge-agent` | Host files |

System paths are checked before the stack folder, so a stack folder inside an
approved root such as `/var` cannot make `./` reach `/var/lib/docker`.

### Never allowed

| Setting | Message |
| --- | --- |
| `build:` | images only |
| `extends`, top-level `include` | reads other files |
| Paths with `..` or starting with `~` | path escapes |
| `env_file` or `label_file` outside the stack folder | must be a file inside the stack folder |
| `provider:` | runs a program on the node |
| Service keys that are not part of the Compose specification (`x-…` extensions are fine) | unknown service key |
| A bind source, `env_file`, `label_file` or config or secret `file` inside another read-write bind mount of the stack | lies inside the read-write bind mount; mount a separate folder |
| A read-write bind mount of the stack folder or a folder that contains it | contains the stack folder (compose.yaml, .env) |
| A stack folder on a system path, such as `/var/lib/docker/…` | the stack folder is a system folder |
| Volume types `npipe`, `cluster` | not supported |
| Top-level keys other than `name`, `services`, `networks`, `volumes`, `configs`, `secrets`, `version` and `x-…` | unknown top-level key |
| `.env` keys starting with `COMPOSE_` or `DOCKER_` | change how Compose runs |

## Limits

| Limit | Value |
| --- | --- |
| Compose file | 512 KiB, one YAML document, depth 32, 50,000 nodes, 2,000 aliases |
| Services | 100 per stack |
| `.env` | 64 KiB |
| Stack and environment names on disk | `[a-z0-9][a-z0-9_-]{0,62}` |
| Service names | `[A-Za-z0-9][A-Za-z0-9_.-]{0,62}` |

YAML is read as YAML 1.2: `yes`, `no`, `on` and `off` are strings, and
numbers with a leading zero (`0755`) stay strings. Duplicate keys are an
error. `version` is ignored.

## Variables

`${VAR}`, `${VAR:-default}`, `${VAR-default}`, `${VAR:?error}`,
`${VAR?error}`, `${VAR:+value}`, `${VAR+value}`, `$VAR` and `$$` work as in
Compose, with values from the stack's `.env` only. The node's environment is
never used.

The policy checks the file after substitution. Previews, policy messages,
port lists and errors show `${VAR}` as written, never its value, so readers
of a stack (`view`) do not see `.env` values.
