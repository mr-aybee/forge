---
title: Spool file format
description: The file name, header and every row kind of the analytics spool files that agents write and upload to the analytics worker.
sidebar:
  order: 11
---

This page describes the files an agent keeps in its analytics spool and
uploads to the analytics worker. Use it to inspect a spool by hand or to
write a test client. For why the spool exists and how it behaves, see
[Analytics: node spools](/docs/concepts/analytics/#node-spools).

The definitions live in `forge_core::spool`
(`src/forge/crates/domain/forge-core/src/spool.rs`).

## Location

| OS | Spool folder |
| --- | --- |
| Linux | `/var/lib/forge-agent/spool/` |
| Windows | `%ProgramData%\OrbynForge\agent\spool\` |

The folder is `spool/` inside the agent's state directory, so
`--state-dir` moves it too.

## File names

```text
<node-id>-<file-id>.ndjson.zst[.<state>]
```

| Part | Format | Meaning |
| --- | --- | --- |
| `<node-id>` | UUID, 36 characters with hyphens | The node that wrote the file |
| `<file-id>` | 32 lowercase hex digits | A UUIDv7 without hyphens. Time ordered, so names sort oldest first |
| `.ndjson.zst` | fixed | zstd-compressed newline-delimited JSON |
| `.<state>` | `tmp`, `ready` or `acked` | Local state on the node; never sent |

The stem (`<node-id>-<file-id>`) is what the agent sends in the
`x-forge-spool-file` header. The worker refuses any other shape, so a name
can never carry a path.

| State | Meaning | At agent start |
| --- | --- | --- |
| `.tmp` | Being written and fsynced | Deleted: it may be partial |
| `.ready` | Sealed and waiting for upload | Kept, unless older than the maximum age |
| `.acked` | The worker acknowledged it | Deleted |

## Limits

| Limit | Value |
| --- | --- |
| Format version | `1` |
| Largest compressed file the worker accepts | 16 MiB (`413` above) |
| Largest decompressed content the worker reads | 64 MiB; it stops reading past this |
| zstd level | 1 to 3 (default 1, `[analytics.spool] zstd_level`) |

## Header

The first line of the decompressed file is the header.

| Field | Type | Meaning |
| --- | --- | --- |
| `v` | integer | Format version, `1` |
| `node` | string | The node id as the node believes it. The worker ignores it and uses the node of the client certificate |
| `created` | integer | When the file was started, epoch ms, node clock |
| `counts` | object | Rows of each kind in the file: `log`, `container_log`, `node_minute`, `top_process`, `container_minute` (integers, missing means `0`) |

```json
{"v":1,"node":"<node-id>","created":1767323045678,"counts":{"log":2,"container_log":0,"node_minute":1,"top_process":0,"container_minute":0}}
```

## Rows

Every later line is one JSON object with its kind in `"k"`.

| `k` | Written by | Stored in |
| --- | --- | --- |
| `log` | The local log listener, one row per request | `app_logs`, `source = ingest` |
| `container_log` | Container output from the root helper | `app_logs`, `source = container` |
| `node_minute` | The agent, one row per minute | Node history (per-minute rows and the hourly rollup) |
| `top_process` | The agent, once a minute | Top processes |
| `container_minute` | Container samples from the root helper | `container_metrics_1m` and the hourly rollup |

### `log`

One request body an application sent to the node's listener, unparsed.

| Field | Type | Meaning |
| --- | --- | --- |
| `key` | string | SHA-256 (hex) of the ingest key in `X-Seq-ApiKey` |
| `ts` | integer | When the node received the request, epoch ms |
| `fmt` | string | `raw`: the body as sent to `/api/events/raw` (CLEF lines or a Seq `{"Events":[…]}` document); the worker tells them apart |
| `body` | string | The request body |

```json
{"k":"log","key":"<sha256-hex>","ts":1767323045678,"fmt":"raw","body":"{\"@t\":\"2026-01-02T03:04:05.678Z\",\"@mt\":\"Job {JobId} done\",\"JobId\":\"j-17\"}\n"}
```

### `container_log`

One line a container wrote.

| Field | Type | Meaning |
| --- | --- | --- |
| `ts` | integer | Docker's timestamp, epoch ms |
| `stream` | string | `stdout` or `stderr` |
| `cid` | string | Short container id |
| `name` | string | Container name |
| `image` | string | Image; may be missing |
| `labels` | object | See [Labels](#labels); may be missing |
| `line` | string | The line |

### `node_minute`

A node's resources over one minute: the averages of its samples.

| Field | Type | Unit |
| --- | --- | --- |
| `minute` | integer | Start of the minute, epoch ms |
| `cpu_avg`, `cpu_max` | number | Percent |
| `mem_used`, `mem_total`, `swap_used` | integer | Bytes |
| `disk_used`, `disk_total` | integer | Bytes |
| `rx`, `tx` | integer | Network bytes per second, averaged |
| `load1` | number | One-minute load average |
| `samples` | integer | Samples averaged |

### `top_process`

One process of the node's top list for one minute.

| Field | Type | Meaning |
| --- | --- | --- |
| `minute` | integer | Start of the minute, epoch ms |
| `pid` | integer | Process id |
| `name` | string | Process name |
| `user` | string | Owner |
| `cpu` | number | CPU percent |
| `mem` | integer | Resident memory, bytes |
| `cmd` | string | Command line |

### `container_minute`

One container's resources for one minute.

| Field | Type | Meaning |
| --- | --- | --- |
| `minute` | integer | Start of the minute, epoch ms |
| `cid` | string | Short container id |
| `name` | string | Container name |
| `image` | string | Image; may be missing |
| `labels` | object | See [Labels](#labels); may be missing |
| `cpu` | number | CPU percent |
| `mem_used`, `mem_limit` | integer | Bytes |
| `rx`, `tx` | integer | Network bytes per second |
| `block_read`, `block_write` | integer | Block I/O bytes per second |
| `restarts` | integer | Restart count |

### Labels

The container labels Forge attributes by. An empty label is left out.

| Field | Container label |
| --- | --- |
| `forge_app` | `forge.app` |
| `forge_stack` | `forge.stack` |
| `compose_project` | `com.docker.compose.project` |
| `compose_service` | `com.docker.compose.service` |

## Trust rules

The worker trusts nothing in the file that names a node or a service:

- The node is the node of the client certificate. The file name must name
  the same node, or the upload is refused with `403`.
- A `log` row counts only when its `key` hash belongs to a service placed
  on that node. Other rows are skipped and counted.
- `forge_app` names a service only when that service is placed on the
  node.

## Inspect a file

On a Linux node with `zstd` installed:

```sh
sudo zstdcat /var/lib/forge-agent/spool/<node-id>-<file-id>.ndjson.zst.ready | head -n 5
```

## Related

- [Agent protocol: analytics spool](/docs/reference/agent-protocol/#analytics-spool)
- [Configuration: `[analytics.spool]`](/docs/reference/configuration/#analyticsspool)
- [Container log collection](/docs/concepts/container-logs/)
