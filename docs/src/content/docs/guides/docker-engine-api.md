---
title: Use a node's Docker Engine API
description: Turn on a node's Docker Engine API over mutual TLS, make Engine API calls through Forge, and turn it off again.
sidebar:
  order: 6
---

This guide turns on the full Docker Engine API for one node and shows you
how to call it through Forge: list, pull, create, logs, events and exec. For
how it works and what the open port means for security, see
[Docker Engine API](/docs/concepts/docker-engine-api/).

## Before you begin

- **Permissions.** Enabling and disabling need `server.command` on the node,
  with an MFA-verified session: **Operator**, **Workspace admin** or
  **Platform admin**. An API key cannot step up, so sign in with
  `forge auth login`.
- **The node.** Linux with systemd and Docker (`docker.service`). Windows
  nodes are not supported.
- **A current agent.** The node's agent must offer the `docker_tls` command
  kind. Update it with `forge agents update <node-name>` if it is older.
- **Network.** The control plane must reach the node on TCP `2376` (or the
  port you choose). Allow it in the node's firewall from the control
  plane's address only.
- **dockerd** does not already listen on TCP through its own configuration
  (`-H tcp://…` or `hosts` in `/etc/docker/daemon.json`).
- **The [forge CLI](/docs/reference/cli/)**, signed in.

:::caution
Enabling restarts dockerd. When `live-restore` is off, running containers
stop; containers with a restart policy start again. Plan a maintenance
window, or turn on `"live-restore": true` in `/etc/docker/daemon.json`
first.
:::

## 1. Enable the endpoint

```sh
forge docker endpoint enable <node-name>
```

Forge dials the address the node's agent connects from. To choose another
address, add `--host <address>`. If that address is not one of the node's
own (it is behind NAT, say), also add `--listen <node-ip>` with the address
dockerd should bind. `--port` changes the port.

If containers are running and `live-restore` is off, the job stops at
`prepare` and tells you how many would stop:

```
error: job 01a1ed4a-9e18-7030-a1a2-a1a7d7665ff4 failed: step `prepare`: enabling restarts dockerd and 3 running container(s) stop (live-restore is off); containers with a restart policy start again. Enable it again with `acknowledge_restart` [job_failed]
```

Run it again with `--acknowledge-restart` to accept the restart:

```sh
forge docker endpoint enable <node-name> --acknowledge-restart
```

```
STATE    HOST           PORT  ENGINE   API   ERROR
enabled  192.0.2.12     2376  29.10.0  1.57  -
```

The CLI waits for the job (up to 10 minutes). Add `--no-wait` to return
once it is queued, then follow it with `forge jobs watch <job-id>`.

## 2. Verify

```sh
forge docker endpoint show <node-name>
forge docker api <node-name> GET /version
```

`show` reports `enabled` with the engine and API versions. The `/version`
call returns dockerd's JSON answer. `forge docker endpoint check
<node-name>` asks dockerd again and records the result; use it to bring a
`failed` endpoint back once you have fixed the cause.

The audit log has `docker.endpoint.enable` for the node.

## 3. Make calls

`forge docker api` sends one Engine API call and streams the answer to
stdout. Paths are as in the
[Docker Engine API reference](https://docs.docker.com/reference/api/engine/),
with an optional query string. Quote paths that contain `?` or `&`.

List containers (`server.manage`):

```sh
forge docker api <node-name> GET '/containers/json?all=1'
```

Pull an image (`container.manage`); the progress streams as JSON lines:

```sh
forge docker api <node-name> POST '/images/create?fromImage=busybox&tag=1.36'
```

```
...
{"status":"Status: Downloaded newer image for busybox:1.36"}
```

Create and start a container (`container.manage`):

```sh
forge docker api <node-name> POST '/containers/create?name=web-test' \
  --data '{"Image":"busybox:1.36","Cmd":["sh","-c","sleep 300"]}'
forge docker api <node-name> POST /containers/web-test/start
```

```
{"Id":"86c1d79aadf4…","Warnings":[]}
```

A body with host access, such as `"HostConfig":{"Privileged":true}` or a
bind mount, needs `server.command` with MFA instead. Read a long body from a
file with `--data @create.json`.

Read logs, and follow the event stream until you press Ctrl-C
(`server.manage`):

```sh
forge docker api <node-name> GET '/containers/web-test/logs?stdout=1&stderr=1&tail=100'
forge docker api <node-name> GET /events
```

Copy files out of a container as a tar archive (`container.exec`):

```sh
forge docker api <node-name> GET '/containers/web-test/archive?path=/etc' --out-file etc.tar
```

### Exec with curl

Exec start upgrades the connection, which `forge docker api` does not do.
For an interactive shell, use the
[container shell](/docs/concepts/container-shells/). To run one command
through the Engine API, use `curl` with HTTP/1.1 and an `Upgrade: tcp` header. Both calls need `container.exec`,
which needs no MFA, so an API key in `FORGE_TOKEN` works:

```sh
FORGE=https://<forge-host>/api/v1
NODE=<node-id>          # from: forge nodes get <node-name>

# 1. Create the exec; the answer is {"Id":"<exec-id>"}
curl -sS -H "Authorization: Bearer $FORGE_TOKEN" -H 'Content-Type: application/json' \
  -d '{"Cmd":["uname","-a"],"AttachStdout":true,"AttachStderr":true,"Tty":true}' \
  "$FORGE/servers/$NODE/docker/engine/containers/web-test/exec"

# 2. Start it on an upgraded connection
curl -sS --http1.1 -N -H "Authorization: Bearer $FORGE_TOKEN" -H 'Content-Type: application/json' \
  -H 'Connection: Upgrade' -H 'Upgrade: tcp' \
  -d '{"Detach":false,"Tty":true}' \
  "$FORGE/servers/$NODE/docker/engine/exec/<exec-id>/start"
```

With `"Tty": true` the output is plain text. Without a TTY, Docker frames
stdout and stderr with an 8-byte header per chunk.

### Use the catalog

Only calls in the [catalog](/docs/reference/docker-api-catalog/) pass. List them,
or only one class:

```sh
forge docker catalog
forge docker catalog --class admin
```

## 4. Disable the endpoint

```sh
forge docker endpoint disable <node-name>
```

```
STATE     HOST  PORT  ENGINE  API  ERROR
disabled  -     -     -       -    -
```

The node removes the listener, restarts dockerd and deletes
`/etc/docker/forge-tls`. The control plane deletes the node's Docker CA, so
no certificate it signed is trusted again. The restart stops running
containers when `live-restore` is off.

If the node is gone or offline for good, forget the endpoint without asking
it:

```sh
forge docker endpoint disable <node-name> --force
```

:::caution
With `--force` nothing changes on the node: dockerd keeps listening on TCP
until you delete `/etc/systemd/system/docker.service.d/forge-tls.conf`
and restart Docker. Nobody can connect, because the CA is gone.
:::

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `step prepare: enabling restarts dockerd and <n> running container(s) stop (live-restore is off) …` | Containers run and `live-restore` is off | Enable again with `--acknowledge-restart`, or turn on `live-restore` first |
| `step apply: dockerd already listens on TCP outside Forge; remove that listener first` | dockerd's command line already has `-H tcp://…` | Remove that listener from the unit or `daemon.json`, restart Docker, then enable again |
| `step apply: dockerd did not come back: … (rolled back; Docker runs as before)` | dockerd failed to start with the TLS listener; the journal lines follow the message | Read the lines in the message, or `journalctl -u docker.service` on the node; fix the cause and enable again |
| ``step sign: `<host>` is not one of the node's addresses (…); pass `listen` …`` | `--host` is a DNS name or a NAT address | Add `--listen <node-ip>` with one of the listed addresses |
| ``no address known for this node: pass `host` …`` | The node has no address on record | Pass `--host <address>` |
| `step verify: dockerd listens, but the control plane cannot reach it: …` | A firewall or routing blocks TCP `2376` from the control plane | Open the port from the control plane's address, then run `forge docker endpoint check <node-name>` |
| `step …: the node did not answer in time` | The node is offline, or its agent predates the `docker_tls` command | Bring the node online; `forge agents update <node-name>` |
| `409` `docker_endpoint_unavailable` | The endpoint is not enabled, still `pending`, or `failed` | `forge docker endpoint show <node-name>` names the state and error; enable it, or run `check` |
| `502` `docker_unreachable` | dockerd does not answer on the endpoint | Check the node is up, Docker runs and the port is open; then `check` |
| `404` `docker_unknown_endpoint` | The method and path are not in the catalog | Check the path against [the catalog](/docs/reference/docker-api-catalog/) |
| `403` `step_up_required` | The call is in the `admin` class, or its body has host access, and your session has no verified second factor | `forge auth step-up`, then retry. API keys cannot make `admin` calls when MFA is required |
| `403` `forbidden` | You do not hold the class's action on the node | Ask for a role with that action; see [Roles and access](/docs/guides/roles-and-access/) |
| `413` `body_too_large` | An inspected JSON body is over 1 MiB | Make the body smaller |
| `400` `this endpoint upgrades the connection …` | Exec start, attach or a BuildKit session without `Upgrade` | Send `Connection: Upgrade` and `Upgrade: tcp` with `curl --http1.1` |
| In Git Bash on Windows, `docker_unknown_endpoint` for a path like `C:/Program Files/Git/containers/json` | MSYS rewrites arguments that start with `/` into Windows paths | Run `export MSYS_NO_PATHCONV=1`, or drop the leading slash: `forge docker api <node-name> GET containers/json` |

## Next steps

- [Docker Engine API](/docs/concepts/docker-engine-api/): certificates, the
  body guard, audit and the trade-off.
- [Docker Engine API catalog](/docs/reference/docker-api-catalog/): every
  forwarded call and the action it needs.
- [`forge docker`](/docs/reference/cli/docker/): every command and flag.
- [Containers](/docs/guides/containers/): the agent path, with no open
  port.
