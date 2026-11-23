---
title: Container log collection
description: How Forge collects the output and resource use of every Docker container on a node, attributes it to services and stacks, and delivers it without loss.
sidebar:
  order: 14
---

Forge collects the output and the resource use of every Docker container on
a Linux node, not only the containers it deployed. This page explains what
is collected, how a line finds its service or stack, and how delivery stays
complete across restarts.

:::caution
Every line a container writes to stdout or stderr is stored in the
analytics database, including lines that contain tokens, passwords or
personal data. Keep secrets out of container output, or leave the container
out of collection (see [Turn collection off](#turn-collection-off)).
:::

## What is collected

| What | Source | Stored in |
| --- | --- | --- |
| Every line on stdout and stderr | Docker's log stream of each running container | `app_logs`, `source = container` |
| CPU, memory (used and limit), network and block I/O rates, restarts | One sample per container and minute | `container_metrics_1m`, rolled up hourly into `container_metrics_1h` |

The root helper (`forge-agent-helper`) watches Docker's `/events` and
follows each container that starts. The agent fetches what the helper
collected on every heartbeat and writes it to its
[spool](/docs/concepts/analytics/#node-spools). The agent uploads the
spool straight to the analytics worker on port `55132`; the lines and
samples never pass through `forge-server`. Collection needs a running
analytics engine: while `[analytics] engine = "off"`, the server tells
nodes not to collect.

Windows nodes do not collect container telemetry.

```diagram
alt: Container telemetry flow
flow:
  rows:
    - steps:
        - { label: "Docker Engine", note: "/events, logs, stats" }
        - { label: "forge-agent-helper (root)", note: "follows containers, buffers, keeps cursors" }
        - { label: "forge-agent", note: "seals lines and samples into the spool" }
        - { label: "forge-analytics-worker", note: "mTLS :55132 · attributes from the client certificate" }
        - { label: "Analytics", note: "app_logs, container_metrics_1m / _1h", tone: accent }
```

## Attribution

The analytics worker decides who a line or sample belongs to. It takes the
node from the client certificate of the upload, never from a value in the
file. Then it
reads the container's labels in this order:

1. `forge.app = <environment>/<name>` names a service, but only a service
   placed on that node. A node cannot write into another node's services.
2. `forge.stack`, else `com.docker.compose.project`, names the stack.
3. Anything else is **unattributed**: it belongs to the node.

| Line belongs to | Who may read it |
| --- | --- |
| A service | `log.read` on the service (or its environment) |
| A stack | `view` on the node |
| Nothing (unattributed) | `view` on the node |

## Levels

Each line becomes a log event, read like this:

- A CLEF line (with `@t`, `@mt` or `@m`) is read as an ingested Seq event.
- Another JSON object gives its level from `@l`, `level`, `Level`, `lvl` or
  `severity`, and its message from `msg`, `message`, `Message` or `MESSAGE`.
  The other keys become properties.
- Plain text is the message. A leading timestamp is dropped, because
  Docker already gives the event its time, and a leading level word
  (`INFO`, `[WARN]`, `error:` and similar) sets the level. A sentence that
  starts with a lower-case word such as "error" keeps it.
- Without a level, a stderr line is `Warning` and a stdout line is
  `Information`.

## Delivery

Delivery is at least once:

- The helper keeps one batch pending until the agent acknowledges it. The
  agent acknowledges a batch only after it has sealed the lines into a
  spool file on disk.
- Only an acknowledged batch moves the container's cursor (a Docker
  timestamp). The cursor lives in the helper's root-only state folder.
- The spool file is deleted only after the worker has stored it and named
  it in its answer. The worker remembers the files it stored, so a file
  sent again is not stored twice.
- A restart of the helper, the agent, the worker or the server neither
  drops nor repeats stored lines. A failure of the worker in the middle of
  storing one file stores part of it twice when the agent resends it.

The helper buffers at most 20,000 lines or 16 MiB. When the buffer is full
it drops the oldest lines and reports how many. A batch holds at most
2,000 lines of up to 16 KiB each. The spool then caps what waits on disk
(512 MiB by default, oldest file dropped first); see
[Node spools](/docs/concepts/analytics/#node-spools).

:::note
Agents of the previous release do not spool. They send container lines
and samples in heartbeats, and the server acknowledges them with
`container_logs_ack` once stored, as before.
:::

## Retention

Container lines share `app_logs` with ingested service logs, so they follow
`[analytics.retention] app_logs_days`. Container samples follow the node
history retentions: per-minute rows `node_metrics_days`, hourly rows
`node_rollup_days`. See [Analytics](/docs/concepts/analytics/) and the
[configuration reference](/docs/reference/configuration/).

## Turn collection off

Collection is on by default for every node. A node setting changes it:
`enabled` turns it off, and `exclude` leaves containers out by name or by a
`key=value` label (at most 64 entries). The server sends the setting back in
every heartbeat answer, so a change takes effect within one heartbeat.
Changing it needs `server.manage` on the node and is audited as
`server.container_telemetry.update`. See
[`PUT /servers/{id}/container-telemetry`](/docs/reference/rest-api/#container-telemetry).

## Trust boundaries

The helper reads Docker as root, but the request that drives it is a closed,
unsigned helper operation, `container_telemetry`: it only reads, and its
settings can only make collection smaller. Everything it reads goes to Forge
through the agent anyway. See
[`forge-agent-helper`](/docs/reference/cli/forge-agent-helper/).

## Related

- [Send and search service logs](/docs/guides/application-logs/)
- [REST API: Service logs](/docs/reference/rest-api/#service-logs)
- [Agent protocol](/docs/reference/agent-protocol/)
- [Spool file format](/docs/reference/spool-file-format/)
