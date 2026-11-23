---
title: Docker Engine API catalog
description: Every Docker Engine API endpoint Forge forwards to a node, with its class, the Forge action it needs, how its body moves and its audit op.
sidebar:
  order: 9
---

This page lists every Docker Engine API call that
`/api/v1/servers/{id}/docker/engine/{path}` forwards to a node's dockerd. Any
other method and path is refused with `404` `docker_unknown_endpoint` before
anything reaches the node. For how the endpoint works, see
[Docker Engine API](/docs/concepts/docker-engine-api/); to turn it on, see
[Use a node's Docker Engine API](/docs/guides/docker-engine-api/).

The same list is served by `GET /api/v1/docker/catalog` and printed by
[`forge docker catalog`](/docs/reference/cli/docker/#forge-docker-catalog).

## How a path matches

- Paths are written as in the
  [Docker Engine API reference](https://docs.docker.com/reference/api/engine/).
  `{}` matches one path segment. `{*}` matches one or more segments, because
  image and plugin names may contain `/` (`/images/ghcr.io/org/app:1/json`).
- Entries without `{*}` are tried first, so `GET /images/json` is the image
  list, not an inspect of an image called `json`.
- A leading version segment such as `/v1.47` is taken off for matching and
  sent on to the daemon. Without one, the daemon answers in its own current
  API version. The list follows Engine API 1.47 and stays valid on newer
  engines.
- Methods are `GET`, `HEAD`, `POST`, `PUT` and `DELETE`. The match ignores
  case in the method.
- Refused with `400` before matching: a path that does not start with `/`,
  is longer than 1,024 characters, contains a control character, space,
  `\`, `?` or `#`, contains `%2f`, `%2e`, `%5c` or `%00` (any case), or has
  an empty, `.` or `..` segment.

## Classes

| Class | Forge action | MFA | Covers |
| --- | --- | --- | --- |
| `read` | `server.manage` | No | Lists, inspects, logs, stats, events. Not audited |
| `manage` | `container.manage` | No | Changes inside Docker's sandbox: containers without host access, images, networks, volumes, prune |
| `exec` | `container.exec` | No | Runs or reads arbitrary things inside a container: exec, attach, archives, export |
| `admin` | `server.command` | Yes | Root on the node or the swarm: swarm, plugins, build, image load, BuildKit sessions, registry login, swarm secrets, configs and services |

The action is checked on the node (`Server`). A request body can raise a
call to `admin`; see [Host access](#host-access). The roles that hold each
action are in the [Cedar schema](/docs/reference/cedar-schema/).

## Transfers

| Transfer | Meaning |
| --- | --- |
| `inspect` | The JSON request body is read, at most 1 MiB, and checked before the call is sent |
| `stream` | Request and response bodies stream both ways (pull progress, logs, events, tar archives). Request bodies up to 8 GiB |
| `hijack` | The connection is upgraded to a raw duplex stream. The request needs `Connection: Upgrade` and `Upgrade: tcp` (or a WebSocket handshake) over HTTP/1.1 |

## Host access

These settings in an `inspect` body raise the call to the `admin` class
(`server.command`, MFA). The refusal and the audit entry name them in
`host_access`.

| Op | Setting | `host_access` value |
| --- | --- | --- |
| `container.create` | `HostConfig.Privileged: true` | `HostConfig.Privileged` |
| `container.create` | `HostConfig.CapAdd` not empty | `HostConfig.CapAdd` |
| `container.create` | `HostConfig.Binds` not empty | `HostConfig.Binds` |
| `container.create` | A `HostConfig.Mounts` entry of `Type` `bind`, `npipe` or `cluster` | `HostConfig.Mounts (bind)` |
| `container.create` | `HostConfig.Devices` not empty | `HostConfig.Devices` |
| `container.create` | `HostConfig.DeviceCgroupRules` not empty | `HostConfig.DeviceCgroupRules` |
| `container.create` | `PidMode`, `IpcMode`, `UTSMode`, `UsernsMode`, `CgroupnsMode` or `NetworkMode` set to `host` | `HostConfig.<Mode>=host` |
| `container.create` | A `HostConfig.SecurityOpt` entry containing `unconfined`, `label=disable` or `label:disable`, or starting with `seccomp` | `HostConfig.SecurityOpt` |
| `exec.create` | `Privileged: true` | `Privileged` |
| `volume.create` | `DriverOpts.device` set, or `DriverOpts.o` containing `bind` | `DriverOpts.device (host path)` |

:::note
Only these three ops are checked for host access. Other `inspect` bodies
(`container.update`, `network.create`, swarm and plugin calls) are read
within the 1 MiB limit and passed on as they are.
:::

## Endpoints

108 endpoints. The **Op** is the operation name in the audit entry
(`details.op` of `docker.api`).

### System

| Method | Path | Class | Forge action | Transfer | Op |
| --- | --- | --- | --- | --- | --- |
| `GET` | `/_ping` | read | `server.manage` | stream | `system.ping` |
| `HEAD` | `/_ping` | read | `server.manage` | stream | `system.ping` |
| `GET` | `/version` | read | `server.manage` | stream | `system.version` |
| `GET` | `/info` | read | `server.manage` | stream | `system.info` |
| `GET` | `/events` | read | `server.manage` | stream | `system.events` |
| `GET` | `/system/df` | read | `server.manage` | stream | `system.df` |
| `POST` | `/auth` | admin | `server.command` | inspect | `system.auth` |

### Containers

| Method | Path | Class | Forge action | Transfer | Op |
| --- | --- | --- | --- | --- | --- |
| `GET` | `/containers/json` | read | `server.manage` | stream | `container.list` |
| `POST` | `/containers/create` | manage | `container.manage` | inspect | `container.create` |
| `POST` | `/containers/prune` | manage | `container.manage` | stream | `container.prune` |
| `GET` | `/containers/{}/json` | read | `server.manage` | stream | `container.inspect` |
| `GET` | `/containers/{}/top` | read | `server.manage` | stream | `container.top` |
| `GET` | `/containers/{}/logs` | read | `server.manage` | stream | `container.logs` |
| `GET` | `/containers/{}/changes` | read | `server.manage` | stream | `container.changes` |
| `GET` | `/containers/{}/export` | exec | `container.exec` | stream | `container.export` |
| `GET` | `/containers/{}/stats` | read | `server.manage` | stream | `container.stats` |
| `POST` | `/containers/{}/resize` | manage | `container.manage` | stream | `container.resize` |
| `POST` | `/containers/{}/start` | manage | `container.manage` | stream | `container.start` |
| `POST` | `/containers/{}/stop` | manage | `container.manage` | stream | `container.stop` |
| `POST` | `/containers/{}/restart` | manage | `container.manage` | stream | `container.restart` |
| `POST` | `/containers/{}/kill` | manage | `container.manage` | stream | `container.kill` |
| `POST` | `/containers/{}/update` | manage | `container.manage` | inspect | `container.update` |
| `POST` | `/containers/{}/rename` | manage | `container.manage` | stream | `container.rename` |
| `POST` | `/containers/{}/pause` | manage | `container.manage` | stream | `container.pause` |
| `POST` | `/containers/{}/unpause` | manage | `container.manage` | stream | `container.unpause` |
| `POST` | `/containers/{}/attach` | exec | `container.exec` | hijack | `container.attach` |
| `GET` | `/containers/{}/attach/ws` | exec | `container.exec` | hijack | `container.attach_ws` |
| `POST` | `/containers/{}/wait` | read | `server.manage` | stream | `container.wait` |
| `DELETE` | `/containers/{}` | manage | `container.manage` | stream | `container.delete` |
| `HEAD` | `/containers/{}/archive` | exec | `container.exec` | stream | `container.archive_info` |
| `GET` | `/containers/{}/archive` | exec | `container.exec` | stream | `container.archive_get` |
| `PUT` | `/containers/{}/archive` | exec | `container.exec` | stream | `container.archive_put` |

### Exec

| Method | Path | Class | Forge action | Transfer | Op |
| --- | --- | --- | --- | --- | --- |
| `POST` | `/containers/{}/exec` | exec | `container.exec` | inspect | `exec.create` |
| `POST` | `/exec/{}/start` | exec | `container.exec` | hijack | `exec.start` |
| `POST` | `/exec/{}/resize` | exec | `container.exec` | stream | `exec.resize` |
| `GET` | `/exec/{}/json` | read | `server.manage` | stream | `exec.inspect` |

### Images

| Method | Path | Class | Forge action | Transfer | Op |
| --- | --- | --- | --- | --- | --- |
| `GET` | `/images/json` | read | `server.manage` | stream | `image.list` |
| `POST` | `/build` | admin | `server.command` | stream | `image.build` |
| `POST` | `/build/prune` | manage | `container.manage` | stream | `build.prune` |
| `POST` | `/images/create` | manage | `container.manage` | stream | `image.create` |
| `GET` | `/images/search` | read | `server.manage` | stream | `image.search` |
| `POST` | `/images/prune` | manage | `container.manage` | stream | `image.prune` |
| `POST` | `/commit` | manage | `container.manage` | stream | `image.commit` |
| `GET` | `/images/get` | read | `server.manage` | stream | `image.get_all` |
| `POST` | `/images/load` | admin | `server.command` | stream | `image.load` |
| `GET` | `/images/{*}/json` | read | `server.manage` | stream | `image.inspect` |
| `GET` | `/images/{*}/history` | read | `server.manage` | stream | `image.history` |
| `POST` | `/images/{*}/push` | manage | `container.manage` | stream | `image.push` |
| `POST` | `/images/{*}/tag` | manage | `container.manage` | stream | `image.tag` |
| `GET` | `/images/{*}/get` | read | `server.manage` | stream | `image.get` |
| `DELETE` | `/images/{*}` | manage | `container.manage` | stream | `image.delete` |
| `GET` | `/distribution/{*}/json` | read | `server.manage` | stream | `distribution.inspect` |

### Networks

| Method | Path | Class | Forge action | Transfer | Op |
| --- | --- | --- | --- | --- | --- |
| `GET` | `/networks` | read | `server.manage` | stream | `network.list` |
| `POST` | `/networks/create` | manage | `container.manage` | inspect | `network.create` |
| `POST` | `/networks/prune` | manage | `container.manage` | stream | `network.prune` |
| `GET` | `/networks/{}` | read | `server.manage` | stream | `network.inspect` |
| `DELETE` | `/networks/{}` | manage | `container.manage` | stream | `network.delete` |
| `POST` | `/networks/{}/connect` | manage | `container.manage` | inspect | `network.connect` |
| `POST` | `/networks/{}/disconnect` | manage | `container.manage` | inspect | `network.disconnect` |

### Volumes

| Method | Path | Class | Forge action | Transfer | Op |
| --- | --- | --- | --- | --- | --- |
| `GET` | `/volumes` | read | `server.manage` | stream | `volume.list` |
| `POST` | `/volumes/create` | manage | `container.manage` | inspect | `volume.create` |
| `POST` | `/volumes/prune` | manage | `container.manage` | stream | `volume.prune` |
| `GET` | `/volumes/{}` | read | `server.manage` | stream | `volume.inspect` |
| `PUT` | `/volumes/{}` | admin | `server.command` | inspect | `volume.update` |
| `DELETE` | `/volumes/{}` | manage | `container.manage` | stream | `volume.delete` |

### Swarm

| Method | Path | Class | Forge action | Transfer | Op |
| --- | --- | --- | --- | --- | --- |
| `GET` | `/swarm` | read | `server.manage` | stream | `swarm.inspect` |
| `POST` | `/swarm/init` | admin | `server.command` | inspect | `swarm.init` |
| `POST` | `/swarm/join` | admin | `server.command` | inspect | `swarm.join` |
| `POST` | `/swarm/leave` | admin | `server.command` | stream | `swarm.leave` |
| `POST` | `/swarm/update` | admin | `server.command` | inspect | `swarm.update` |
| `GET` | `/swarm/unlockkey` | admin | `server.command` | stream | `swarm.unlockkey` |
| `POST` | `/swarm/unlock` | admin | `server.command` | inspect | `swarm.unlock` |

### Nodes

| Method | Path | Class | Forge action | Transfer | Op |
| --- | --- | --- | --- | --- | --- |
| `GET` | `/nodes` | read | `server.manage` | stream | `node.list` |
| `GET` | `/nodes/{}` | read | `server.manage` | stream | `node.inspect` |
| `DELETE` | `/nodes/{}` | admin | `server.command` | stream | `node.delete` |
| `POST` | `/nodes/{}/update` | admin | `server.command` | inspect | `node.update` |

### Services

| Method | Path | Class | Forge action | Transfer | Op |
| --- | --- | --- | --- | --- | --- |
| `GET` | `/services` | read | `server.manage` | stream | `service.list` |
| `POST` | `/services/create` | admin | `server.command` | inspect | `service.create` |
| `GET` | `/services/{}` | read | `server.manage` | stream | `service.inspect` |
| `DELETE` | `/services/{}` | admin | `server.command` | stream | `service.delete` |
| `POST` | `/services/{}/update` | admin | `server.command` | inspect | `service.update` |
| `GET` | `/services/{}/logs` | read | `server.manage` | stream | `service.logs` |

### Tasks

| Method | Path | Class | Forge action | Transfer | Op |
| --- | --- | --- | --- | --- | --- |
| `GET` | `/tasks` | read | `server.manage` | stream | `task.list` |
| `GET` | `/tasks/{}` | read | `server.manage` | stream | `task.inspect` |
| `GET` | `/tasks/{}/logs` | read | `server.manage` | stream | `task.logs` |

### Secrets

| Method | Path | Class | Forge action | Transfer | Op |
| --- | --- | --- | --- | --- | --- |
| `GET` | `/secrets` | read | `server.manage` | stream | `secret.list` |
| `POST` | `/secrets/create` | admin | `server.command` | inspect | `secret.create` |
| `GET` | `/secrets/{}` | read | `server.manage` | stream | `secret.inspect` |
| `DELETE` | `/secrets/{}` | admin | `server.command` | stream | `secret.delete` |
| `POST` | `/secrets/{}/update` | admin | `server.command` | inspect | `secret.update` |

### Configs

| Method | Path | Class | Forge action | Transfer | Op |
| --- | --- | --- | --- | --- | --- |
| `GET` | `/configs` | read | `server.manage` | stream | `config.list` |
| `POST` | `/configs/create` | admin | `server.command` | inspect | `config.create` |
| `GET` | `/configs/{}` | read | `server.manage` | stream | `config.inspect` |
| `DELETE` | `/configs/{}` | admin | `server.command` | stream | `config.delete` |
| `POST` | `/configs/{}/update` | admin | `server.command` | inspect | `config.update` |

### Plugins

| Method | Path | Class | Forge action | Transfer | Op |
| --- | --- | --- | --- | --- | --- |
| `GET` | `/plugins` | read | `server.manage` | stream | `plugin.list` |
| `GET` | `/plugins/privileges` | read | `server.manage` | stream | `plugin.privileges` |
| `POST` | `/plugins/pull` | admin | `server.command` | inspect | `plugin.pull` |
| `POST` | `/plugins/create` | admin | `server.command` | stream | `plugin.create` |
| `GET` | `/plugins/{*}/json` | read | `server.manage` | stream | `plugin.inspect` |
| `POST` | `/plugins/{*}/enable` | admin | `server.command` | stream | `plugin.enable` |
| `POST` | `/plugins/{*}/disable` | admin | `server.command` | stream | `plugin.disable` |
| `POST` | `/plugins/{*}/upgrade` | admin | `server.command` | inspect | `plugin.upgrade` |
| `POST` | `/plugins/{*}/push` | admin | `server.command` | stream | `plugin.push` |
| `POST` | `/plugins/{*}/set` | admin | `server.command` | inspect | `plugin.set` |
| `DELETE` | `/plugins/{*}` | admin | `server.command` | stream | `plugin.delete` |

### BuildKit sessions

| Method | Path | Class | Forge action | Transfer | Op |
| --- | --- | --- | --- | --- | --- |
| `POST` | `/session` | admin | `server.command` | hijack | `session.start` |
| `POST` | `/grpc` | admin | `server.command` | hijack | `session.grpc` |
