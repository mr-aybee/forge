---
title: forge docker
description: Enable, check and disable a node's Docker Engine API over mutual TLS, list the forwarded calls, and make one Engine API call through Forge.
sidebar:
  order: 9
---

`forge docker` manages a node's Docker Engine API endpoint (dockerd on TCP
with mutual TLS) and makes Engine API calls through the control plane. For
how it works, see [Docker Engine API](/docs/concepts/docker-engine-api/); for the
task, see [Use a node's Docker Engine API](/docs/guides/docker-engine-api/).
Every command takes the [global flags](/docs/reference/cli/#global-flags).

Who may run what: `endpoint show`, `endpoint check` and `read`-class calls
need `server.manage` on the node. `endpoint enable` and `endpoint disable`
need `server.command` with an MFA-verified session. Engine API calls need
the action of their class; see the
[catalog](/docs/reference/docker-api-catalog/#classes). `catalog` needs `view`.

The sample output below comes from a lab node `lab-c` (addresses changed to
the `192.0.2.0/24` documentation range).

## Commands

| Command | What it does |
| --- | --- |
| [`forge docker endpoint show`](#forge-docker-endpoint-show) | Shows the endpoint's state, address, versions and last error |
| [`forge docker endpoint enable`](#forge-docker-endpoint-enable) | Turns the endpoint on: dockerd restarts and listens on TCP with mutual TLS |
| [`forge docker endpoint disable`](#forge-docker-endpoint-disable) | Turns it off and forgets the node's Docker CA |
| [`forge docker endpoint check`](#forge-docker-endpoint-check) | Asks dockerd now and records its version or the failure |
| [`forge docker catalog`](#forge-docker-catalog) | Lists every Engine API call Forge forwards, with the Forge action it needs |
| [`forge docker api`](#forge-docker-api) | Makes one Engine API call on a node and streams the answer |

Exit codes follow the [common exit codes](/docs/reference/cli/#exit-codes):
`0` success, `2` invalid flags, a bad path or a `400`/`413` answer, `3` not
signed in, `4` the action is denied, `5` a second factor is needed, `6` no
such node or the path is not in the catalog, `7` the endpoint is not
enabled (`docker_endpoint_unavailable`), `9` the server or dockerd is
unreachable (`502`), `10` the enable or disable job failed.

### Endpoint table

`show`, `check`, `enable` and `disable` print the endpoint with these
columns. `-o json` prints every field of the
[endpoint object](/docs/reference/rest-api/#docker-engine-api).

| Column | Field | Meaning |
| --- | --- | --- |
| `STATE` | `state` | `disabled`, `pending`, `enabled` or `failed` |
| `HOST` | `host` | The address the control plane dials |
| `PORT` | `port` | The TCP port dockerd listens on |
| `ENGINE` | `engine_version` | Docker's version, from `/version` |
| `API` | `api_version` | Docker's Engine API version |
| `ERROR` | `last_error` | Why the last enable or check failed |

### forge docker endpoint show

```sh
forge docker endpoint show <node>
```

| Argument | Type | Meaning |
| --- | --- | --- |
| `<node>` | string, required | Node id, host name or name |

```
$ forge docker endpoint show lab-c
STATE    HOST        PORT  ENGINE   API   ERROR
enabled  192.0.2.12  2376  29.10.0  1.57  -
```

A node without the endpoint shows `disabled` with every other column `-`.

### forge docker endpoint enable

Starts a `docker.endpoint.enable` job (steps `prepare`, `sign`, `apply`,
`verify`), waits for it and prints the endpoint. Enabling restarts dockerd.
Audited as `docker.endpoint.enable`.

```sh
forge docker endpoint enable <node> [--host <address>] [--port <port>] [--listen <ip>] [--acknowledge-restart] [--no-wait]
```

| Argument or flag | Type | Default | Meaning |
| --- | --- | --- | --- |
| `<node>` | string, required | | Node id, host name or name |
| `--host` | string | The address the node's agent connects from | Address the control plane dials: an IP address or a DNS name |
| `--port` | integer | `2376` | TCP port dockerd listens on; `1024` or higher |
| `--listen` | IP address | `--host`, when it is one of the node's addresses | Address dockerd binds. `0.0.0.0` listens on all addresses. Loopback and multicast are refused |
| `--acknowledge-restart` | flag | off | Accept that running containers stop while dockerd restarts (`live-restore` off). Without it the job fails at `prepare` when containers run |
| `--no-wait` | flag | off | Print `{job_id}` once the job is queued instead of waiting (the wait is up to 10 minutes) |

```
$ forge docker endpoint enable lab-c
error: job 01a1ed4a-9e18-7030-a1a2-a1a7d7665ff4 failed: step `prepare`: enabling restarts dockerd and 3 running container(s) stop (live-restore is off); containers with a restart policy start again. Enable it again with `acknowledge_restart` [job_failed]

$ forge docker endpoint enable lab-c --acknowledge-restart
STATE    HOST        PORT  ENGINE   API   ERROR
enabled  192.0.2.12  2376  29.10.0  1.57  -
```

```sh
# Dial a NAT address, bind the node's private address, use port 12376
forge docker endpoint enable <node> --host 203.0.113.7 --listen 10.0.0.5 --port 12376 --acknowledge-restart
```

### forge docker endpoint disable

Starts a `docker.endpoint.disable` job (steps `remove`, `forget`), waits for
it and prints the endpoint. The node removes the listener and restarts
dockerd; the control plane deletes the node's Docker CA. Audited as
`docker.endpoint.disable`. Fails with exit code `7` when the endpoint is not
enabled.

```sh
forge docker endpoint disable <node> [--force] [--no-wait]
```

| Argument or flag | Type | Default | Meaning |
| --- | --- | --- | --- |
| `<node>` | string, required | | Node id, host name or name |
| `--force` | flag | off | Forget the endpoint without asking the node (when the node is gone). The node keeps its listener until you remove `/etc/systemd/system/docker.service.d/forge-tls.conf` |
| `--no-wait` | flag | off | Print `{job_id}` once the job is queued instead of waiting |

```
$ forge docker endpoint disable lab-c
STATE     HOST  PORT  ENGINE  API  ERROR
disabled  -     -     -       -    -
```

### forge docker endpoint check

Calls `/version` on dockerd now and records the versions, or the error, on
the endpoint. A `pending` or `failed` endpoint becomes `enabled` when dockerd
answers. Fails with exit code `7` when the endpoint is not enabled.

```sh
forge docker endpoint check <node>
```

| Argument | Type | Meaning |
| --- | --- | --- |
| `<node>` | string, required | Node id, host name or name |

```
$ forge docker endpoint check lab-c
STATE    HOST        PORT  ENGINE   API   ERROR
enabled  192.0.2.12  2376  29.10.0  1.57  -
```

### forge docker catalog

Lists every Engine API call Forge forwards (`GET /api/v1/docker/catalog`).
The full list is in the [Docker Engine API catalog](/docs/reference/docker-api-catalog/).

```sh
forge docker catalog [--class <class>]
```

| Flag | Type | Default | Meaning |
| --- | --- | --- | --- |
| `--class` | `read`, `manage`, `exec` or `admin` | all | Only calls of this class |

Columns: `METHOD`, `PATH`, `CLASS`, `ACTION` (the Forge action before body
checks) and `OP` (the audit op). `-o json` adds `transfer`.

```
$ forge docker catalog --class admin
METHOD  PATH                  CLASS  ACTION          OP
POST    /auth                 admin  server.command  system.auth
POST    /build                admin  server.command  image.build
POST    /images/load          admin  server.command  image.load
...
```

### forge docker api

Makes one Engine API call on a node through the control plane and streams
the answer to stdout, or to `--out-file`. There is no overall time limit, so
`GET /events` and logs with `follow=1` run until you press Ctrl-C. Calls
outside the `read` class are audited as `docker.api`.

```sh
forge docker api <node> <method> <path> [--data <JSON>|--upload <FILE>] [--out-file <FILE>]
```

| Argument or flag | Type | Default | Meaning |
| --- | --- | --- | --- |
| `<node>` | string, required | | Node id, host name or name |
| `<method>` | string, required | | `GET`, `HEAD`, `POST`, `PUT` or `DELETE` (any case) |
| `<path>` | string, required | | Engine API path with an optional version prefix and query string: `/containers/json?all=1`, `/v1.47/images/nginx:1.27/json`. The leading `/` is optional |
| `--data` | JSON string, or `@file` | none | JSON request body, sent as `application/json`. `@create.json` reads it from a file. Cannot be used with `--upload` |
| `--upload` | file path | none | Raw request body from a file, sent as `application/x-tar` (archive uploads, image loads, builds) |
| `--out-file` | file path | stdout | Write the answer to this file |

Attach, exec start and BuildKit sessions upgrade the connection and do not
work here; see [Exec with curl](/docs/guides/docker-engine-api/#exec-with-curl).
On a `4xx` or `5xx` answer the CLI prints the error and exits with the
matching code.

```
$ forge docker api lab-c POST '/images/create?fromImage=busybox&tag=1.36'
...
{"status":"Status: Downloaded newer image for busybox:1.36"}

$ forge docker api lab-c POST '/containers/create?name=web-test' --data '{"Image":"busybox:1.36","Cmd":["sh","-c","sleep 300"]}'
{"Id":"86c1d79aadf4…","Warnings":[]}

$ forge docker api lab-c GET /containers/x/secret
error: not a Docker Engine API path Forge forwards: GET /containers/x/secret [docker_unknown_endpoint]
```

More examples:

```sh
forge docker api <node> GET '/containers/json?all=1'
forge docker api <node> GET '/containers/web-test/logs?stdout=1&stderr=1&tail=100'
forge docker api <node> GET '/containers/web-test/archive?path=/etc' --out-file etc.tar
forge docker api <node> PUT '/containers/web-test/archive?path=/tmp' --upload files.tar
forge docker api <node> DELETE '/containers/web-test?force=1'
```

:::tip
In Git Bash on Windows, MSYS turns an argument that starts with `/` into a
Windows path. Run `export MSYS_NO_PATHCONV=1`, or leave out the leading
slash: `forge docker api <node> GET version`.
:::
