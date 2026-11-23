---
title: Analytics and the two databases
description: Why Forge keeps operational data and analytics data in separate databases, how nodes spool and upload analytics data, and which engines each database supports.
sidebar:
  order: 40
---

Forge keeps two kinds of data with very different shapes, so it keeps them in
two databases.

| | Operational database | Analytics database |
| --- | --- | --- |
| Holds | Users, groups, roles, nodes, apps, environments, services, jobs, audit log, settings, log ingest keys | Node history (per-minute metrics, the top processes and hourly trends) and service logs |
| Shape | Small rows, many updates, relations and transactions | Append-only rows in time order, very many of them, read in ranges and aggregates |
| Engines | SQLite (default) or PostgreSQL | Embedded (ClickHouse run by Forge), a ClickHouse server, DuckDB (inside the worker), or off |
| Default name | `forge_op` | `forge_op_analytics` |
| Losing it | Forge cannot run | Forge keeps working; history and log pages say analytics is unavailable |
| Served by | `forge-server` | `forge-analytics-worker`, a child process of `forge-server`; nodes upload to it directly |

## Why two databases

A transactional database is built for small, consistent changes. Time-series
data grows without stopping: one busy service can write hundreds of log
events a second. Putting that into the operational database makes it large,
slows backups and vacuum, and lets a burst of logs slow down deployments and
access checks.

A column store such as ClickHouse compresses this data 10 to 20 times,
deletes old data cheaply by expiring whole parts, and answers range and
aggregate queries quickly. The embedded engine is ClickHouse run by Forge
itself as a supervised process on `127.0.0.1`, so a single-box install gets
the same engine without managing a server. The embedded engine and a
ClickHouse server use the same tables and the same SQL.

Forge runs the embedded engine lean. It turns off the engine's own system
log tables (query, trace, metric, part and similar logs) and its query
profiler, keeps only its crash log (with a 6-hour TTL), and reads the engine's internal metrics once a minute. These
settings live in `<data_dir>/analytics/config.d/forge-quiet.xml`, which Forge
rewrites when it starts the engine. An idle engine then uses about a tenth of
one CPU core and about 100 MiB of memory, plus the memory-mapped binary.

## The DuckDB store

`engine = "duckdb"` keeps analytics in DuckDB, an in-process column store,
inside the analytics worker itself. There is no engine process, no port and
no download, and an idle worker does no work. It is the default engine; see
[Choosing engines](#choosing-engines) for when to use another.

The store keeps DuckDB's own file format under
`<data_dir>/analytics/duckdb`, split into **one database file per UTC day**
and per family of tables:

| Folder | Tables | One file per | Kept for |
| --- | --- | --- | --- |
| `logs/` | `app_logs` | day | `app_logs_days` |
| `metrics/` | `node_minute`, `container_minute` | day | `node_metrics_days` |
| `processes/` | `top_process` | day | `top_processes_days` |
| `rollups/` | `node_1h`, `container_1h` | month | `node_rollup_days` |

`meta.duckdb` holds the settings, the time range each file covers, and how
far the rollups have read. Temporary files of large queries go to `tmp/`.

**Rows go to the file of the day they arrive**, not the day they happened.
One batch therefore writes exactly one file per family, and its rows commit
in one transaction together with the ids of the spool files they came from.
The time range of each file's events is recorded before its rows commit, so
a search opens only the files whose range overlaps its window. A late spool
file from a node that was offline lands in today's file and still shows up
in searches of the day it happened. Inside a file, each batch is written in
time order, so DuckDB's per-block minimum and maximum skip most of a file
for a narrow window.

**Each spool file is stored once per table.** When a node sends a file again
(its acknowledgement was lost), each family that already holds the file's
id skips it. A crash between two families' commits leaves the second one
without the file; the node sends the file again, and only the missing rows
are stored. The worker keeps the last 16 file ids per node and family in
memory and reads them back from the last 4 days' files when it starts.

**One writer.** Every write goes through one thread: spool files, other
writes and the hourly jobs. It takes every upload waiting (up to
`ingest.max_batch_rows` rows) and appends them with DuckDB's appender in one
transaction per family. Nothing waits on a timer: a lone upload is written
at once, and under load the uploads that arrive while one batch commits form
the next. Searches and history reads run on their own connections, at most
as many at once as DuckDB has threads, so they cannot crowd out the writer.

**Hourly jobs**, a minute after start and then every hour; their last run,
duration and error show on the Analytics settings page:

- **Rollups.** Minute rows not yet rolled up become hourly states (sums,
  counts, maxima and the latest container identity) in the month's rollup
  file. A late minute adds a state to its hour. Hourly history combines the
  states with the minute rows that arrived since the last run, so it is
  complete without waiting for the job.
- **Retention.** A file past its family's retention is detached and deleted
  with its write-ahead log. Disk is freed at once; there are no row deletes
  and no `VACUUM`. Minute files are rolled up before they go.
- **Compaction.** Files of past days are checkpointed (their write-ahead log
  folded in) and detached when no query has used them for 10 minutes.

:::note
Retention counts from the day a row arrived, so a row can stay up to a day
longer than its retention, plus the time it waited in a node's spool. With
a retention under 4 days, a node that sends a file again after the file's
day was dropped stores it a second time.
:::

**Limits.** DuckDB's memory limit is `[analytics] duckdb_memory_mb`
(setting `memory_budget_mb`, default 256 MiB) and its threads follow the CPU
cap (`cpu_quota_vcpu` rounded up, 2 when no cap is set). Both apply at once
when changed. They belong to the whole DuckDB instance: the per-search
threads and memory settings have no effect, and the search timeout
interrupts the query. Because DuckDB runs inside the worker, the worker's
memory limit must leave room for it (`worker_memory_limit_mb` at least
`duckdb_memory_mb` plus 256).

## The analytics worker process

All analytics work runs in one process, `forge-analytics-worker`, not in
`forge-server`. The worker owns the analytics store, the per-minute
buffers, the log batch writer, log searches, retention, the runtime
settings, the spool upload listener for nodes and, for the embedded
engine, the engine process itself. `forge-server` stays a control plane:
it authenticates and authorizes every request, writes the audit log, and
passes data to the worker without decoding it. Node data does not pass
through `forge-server` at all.

```
apps on a node ──:5341──▶ forge-agent ──spool files, mTLS :55132──────────────▶ forge-analytics-worker ──▶ engine
                            │ heartbeats: live sample, spool status              ▲  import, buffers,
                            ▼                                                    │  batch writer,
off-fleet apps ──/api/events/raw──▶ forge-server ──control socket (0600 + secret)┘  searches, retention
console ──/api/v1/logs…──────────▶  authorize, audit, maps
```

- **One private channel.** `forge-server` starts the worker and talks to
  it over a Unix socket in `<data_dir>/analytics-worker/` (mode 0600, in a
  directory only the service user can enter). Every connection first
  presents a random secret that `forge-server` hands the worker on its
  standard input at each start, so no secret is written to disk. On
  Windows the channel is loopback TCP with the same secret.
- **Maps, not database access.** The worker never reads the operational
  database. `forge-server` pushes what the worker needs to attribute data:
  the active nodes, which services run on which nodes and under which
  names, and the ingest key hashes. It pushes them when they change and
  whenever the worker restarts.
- **Supervised.** `forge-server` restarts the worker with back-off when
  it exits, when its socket stops answering three health checks in a row,
  or when its resident memory passes `[analytics] worker_memory_limit_mb`.
  Each restart is written to the audit log
  (`analytics.worker.restart`) and shown on the Analytics settings page.
- **Failure stays contained.** While the worker is down, history, log
  search and off-fleet log ingest answer `503 analytics_worker_down`, and
  senders retry. Nodes keep writing to their spools and upload when the
  worker is back. Deployments, access checks, jobs and the rest of the console keep
  working. The worker exits when `forge-server` stops, so it never
  outlives it.
- **Bounded.** Every buffer and queue in the worker has a fixed size and a
  counted drop or an immediate refusal. `forge-server` keeps no analytics
  buffers or queues.

A worker restart loses nothing a node sent: a spool file is deleted only
after the worker acknowledged it, and an off-fleet log batch that was
waiting was not acknowledged either, so its sender sends it again. The
embedded engine is a child of the worker and restarts with it.

## Node spools

Every agent keeps a spool on its own disk and uploads it straight to the
analytics worker. The control plane only handles liveness, commands and
settings; analytics volume grows with the fleet without loading
`forge-server`.

```diagram
alt: Analytics data flow from a node
flow:
  rows:
    - steps:
        - { label: "Apps, Docker, the node", note: "POST :5341 · container output · minute samples" }
        - { label: "Spool on the node", note: "write buffer → sealed .zst files", tone: accent }
        - { label: "Analytics worker", note: "mTLS :55132 · checks, parses, stores" }
        - { label: "Analytics database", note: "app_logs · node history · container metrics" }
```

**What goes into the spool.** Five sources write rows, and the agent does
not parse any of them:

| Source | Row kind |
| --- | --- |
| Requests apps send to the node's local log listener (`127.0.0.1:5341`, and the Docker bridge gateway for containers) | `log`: the request body as received, with the hash of its key |
| Container output from the root helper | `container_log` |
| The node's resource samples, folded per minute | `node_minute` |
| The top processes by CPU and by memory, once a minute | `top_process` |
| Container resource samples from the root helper | `container_minute` |

Heartbeats carry only the live sample for the **Overview** charts, the
spool's status and command results. Their answers bring the worker's URL,
the spool and listener limits, and the hashes of the ingest keys of the
services placed on that node.

**Sealing and upload.** Rows go into one write buffer in memory. The agent
seals it at 4 MiB or 10 seconds: it compresses it with zstd into a `.tmp`
file, fsyncs it, renames it to `.ready` and fsyncs the folder. An uploader
sends ready files oldest first, one at a time, over HTTP/2 with the node's
client certificate. A file is deleted only after the worker's answer names
it. The format is in the
[spool file format reference](/docs/reference/spool-file-format/).

**Back-pressure.** Every stage has a fixed limit and says no instead of
growing:

- The worker imports at most `[analytics] worker_max_uploads` files at
  once (default 8, up to 32). One more upload is answered `429` at once; the agent
  backs off with jitter (1 to 60 seconds) and the spool absorbs the
  backlog.
- The spool has a size cap (512 MiB), a free-disk floor (256 MiB) and a
  maximum age (72 hours). While it is full or the disk is below the
  floor, the local listener answers `429` and apps keep their events and
  retry.
- The listener caps a request body (1 MiB) and, on the Docker bridge
  address, the request rate per key (50 a second, burst 100).
- A container line is sealed to disk before the helper is acknowledged,
  so the helper's cursor never passes a line that is not on disk.

**What can be lost.** The spool is durable once a file is sealed. These
are the losses it accepts:

| Event | What is lost |
| --- | --- |
| The node crashes or loses power | Rows in the write buffer: at most the seal interval (10 s by default) of application logs and samples. Container lines are not lost: the helper sends them again |
| The spool reaches its cap (worker unreachable for a long time) | The oldest sealed file, counted in `SpoolStatus.dropped_files` |
| A file is older than the maximum age | That file, counted the same way |
| The worker refuses a file as unreadable (`400`, `413`, `422`) | That file; the agent logs a warning |

**Trust.** The worker trusts nothing in a file that names a node or a
service. The node is the node of the client certificate; the file name
must name the same node, and the node must be active. The certificate must
also be the node's current one: `forge-server` sends the serial of each
active node's certificate with the maps it pushes to the worker, so a
certificate replaced at renewal, or one of a revoked or removed node, is
refused with `403`. Renewal, revocation and removal push the maps at once.
A `log` row counts only if its key belongs to a service placed on that
node, and a `forge.app` label names only a service placed there. Other
rows are skipped and counted. A node therefore cannot write into another
node's history or into a service that does not run on it.

**The listener's certificate.** `forge-server` issues the upload
listener's certificate from Forge's internal CA (valid 90 days) each time
it starts the worker, and hands it over on the worker's standard input; it
is never written to disk. At two thirds of its life `forge-server` issues
a new one and sends it to the running worker over the control socket. New
connections get the new certificate at once, open ones keep theirs, and
the worker does not restart. If the hand-over fails, it is tried again
every 5 seconds, and the reason shows as the worker's last error on the
Analytics settings page. With less than a day left and still no renewal,
`forge-server` restarts the worker, which hands it a fresh certificate.

**Stored once.** The worker records the id of every stored file in
`<data_dir>/analytics-worker/spool-ids/seen.log`, the last 64 per node,
and keeps the record across restarts. A file sent again after a lost
answer is acknowledged as a duplicate and not stored again. How a file
that failed part-way is stored once depends on the store: DuckDB records
file ids with the rows (see [The DuckDB store](#the-duckdb-store)), and
ClickHouse and the embedded engine use deduplication tokens (see
[Spool files in ClickHouse](#spool-files-in-clickhouse)).

### Spool files in ClickHouse

With `clickhouse` and `embedded`, one writer task in the worker stores the
spool files:

- **Grouped inserts.** It takes the first file waiting and every file
  queued behind it, up to `ingest.max_batch_rows` rows (setting
  `ingest_max_batch_rows`, default 20,000) or 256 files, and writes them
  with one insert per table, the four tables at once. Nothing waits on a
  timer: a lone file is written at once, and under load the files that
  arrive while one batch is written form the next. Each file is answered
  only after every table it has rows in confirmed its insert.
- **Once per table.** Each insert carries an `insert_deduplication_token`
  built from the file ids it holds and the table name (`<file id>:<table>`
  for one file). The tables keep a deduplication window of 10,000 inserts
  (`non_replicated_deduplication_window`), the hourly rollups included, so
  an insert sent again with the same token is acknowledged without
  writing. A failed insert is retried at once with the same rows and
  token, so an insert that landed although its answer was lost is not
  written twice. If a table still fails, the worker remembers which tables
  each file already reached, and when the node sends the file again only
  the missing tables are written.

:::note
One case is still at least once: the worker stops (crash or restart)
between two table inserts of a batch of several files. The nodes send
those files again, they form other batches with other tokens, and the
tables already written get those rows twice. A file stored on its own,
the usual case at light load, is covered by its token even then.
:::

**Old agents.** Agents from the previous release do not spool. They still
send container lines, container samples and top processes in heartbeats,
and `forge-server` forwards them to the worker as before.

## How node history is recorded

The agent folds its resource samples into one row per minute and, once a
minute, records the ten processes using the most CPU and the ten using the
most memory. Both go into its spool, and the worker writes them when the
file arrives. A materialized view keeps an hourly rollup (with DuckDB, the
hourly rollup job; see [The DuckDB store](#the-duckdb-store)). Per-minute rows and top processes are kept 7 days and hourly
rows 365 days by default; ClickHouse drops expired data by itself.

If the analytics database cannot be reached, files wait in the node's
spool and are uploaded when it is back. Heartbeats never wait for it.

## How service logs are stored

Services send structured events with the Seq protocol (CLEF lines or
`{"Events":[...]}`). Each request carries an ingest key, and the key decides
the service: an event cannot name another service, so a key leaks at most
the right to write into one stream. Keys live in the operational database
as SHA-256 hashes; events live in the `app_logs` table of the analytics
database. There are two ways in:

| Sender | Sends to | Path to the worker |
| --- | --- | --- |
| A service Forge runs on a node | The node's local listener, `FORGE_LOG_URL` (`http://127.0.0.1:5341`, or `http://forge-node:5341` in a container) | The agent checks the key against the keys of services placed on that node, appends the body to its spool unparsed and answers `201`. The worker parses the body when the file arrives |
| An app outside the fleet | `forge-server`, `POST /api/events/raw` | `forge-server` checks the key and passes the body, as received, to the worker over the control socket |

Both paths use the same parser in the worker, so an event is stored the
same way whichever path it took. An event from a node's listener also
gets the node: log filters by node work for it.

On the off-fleet path, one writer collects the batches of concurrent
requests and stores them in a single insert, and each request is answered
once its events are stored. The queue in front of the writer is bounded
by request count and by the size of the parsed events. When it is full,
when the engine is short of disk, or when the analytics database or the
worker is down, the request fails with `503` at once and the sender
retries. A batch whose sender has stopped waiting is dropped before the
insert, so the retry does not store it twice. Nothing outlives the answer.

:::note
Delivery is at least once on both paths. Off the fleet, if the insert has
already started when the sender's request times out, the retry can store
the batch a second time. On a node, the listener answers `201` once the
body is in the write buffer, so a crash of the node can lose up to the
seal interval of events (see [Node spools](#node-spools)).
:::

Each event keeps three timestamps:

| Field | Set by | Used for |
| --- | --- | --- |
| `ts` | The app (`@t`); the ingest time when the app sends none | Ordering, paging, the histogram and time filters |
| `node_ts` | The node's agent when its listener receives the request | Recorded with the event; empty for events sent straight to the server |
| `ingest_ts` | Forge, when it receives the event | Retention |

Retention counts from `ingest_ts`, so an app with a wrong clock cannot keep
data longer or lose it early. Events are kept 30 days by default
(`[analytics.retention] app_logs_days`); ClickHouse drops expired days by
itself.

The same table holds the output of every Docker container, collected by
the nodes (`source = container`, with `container_id`, `container_name`,
`stack` and `stream`); see
[Container log collection](/docs/concepts/container-logs/). Ingested
events have `source = ingest`. Rows are ordered by node, then time, with a
skip index on `service_id`. The server always names what a query may read:
the services on which the caller holds `log.read`, and for container lines
that belong to no service, the nodes on which the caller holds `view`.
Nobody reads a service's logs through a wider query.

Container resource samples go to `container_metrics_1m` (one row per
container and minute: CPU, memory used and limit, network and block I/O,
restarts, with the container's service or stack) and an hourly rollup,
`container_metrics_1h`. They follow the node history retentions:
`container_metrics_1m` keeps rows for the **Minute-level history** setting
(`node_metrics_days`), and `container_metrics_1h` for **Hourly rollups**
(`node_rollup_days`).

Forge renders the message from the template and properties when the app
sends no rendered message, and maps level names from Serilog, NLog,
Microsoft.Extensions.Logging, log4net, Python and syslog onto six levels:
Verbose, Debug, Information, Warning, Error and Fatal. Unknown names become
Information.

## Runtime settings and resource limits

Retention and resource limits are settings that change while Forge runs.
`forge.toml` supplies the defaults. A platform administrator's overrides
live in the operational database, so they survive restarts and upgrades
and do not require editing files on the host. Each change is applied and
then read back, and the result is reported per setting. A partial success
therefore shows what took effect and what did not.

Where a setting takes effect decides how it is applied:

| Setting kind | Where it acts | Embedded engine | External ClickHouse | DuckDB |
| --- | --- | --- | --- | --- |
| Retention | Table TTLs; DuckDB: whole day files | Applied at once | Applied at once (needs `ALTER` on the tables) | Expired files are dropped at once |
| CPU cap | The engine's CPU affinity and thread pools | Pinning applies at once; pools resize at the next engine restart | Not configurable from Forge | DuckDB's `threads`, at once |
| Memory budget | The engine's configuration file | Picked up within seconds, or at the next engine restart | Not configurable from Forge | DuckDB's `memory_limit`, at once |
| Search, ingest and disk limits | The analytics worker | Applied at once | Applied at once | Applied at once; per-search threads and memory have no effect |

The CPU and memory limits apply to the embedded engine only, never to
`forge-server` or the worker, and need no privileges, systemd delegation
or cgroup changes. The worker's own memory is watched separately (see
[The analytics worker process](#the-analytics-worker-process)):

- **Memory.** The memory budget is the engine's `max_server_memory_usage`.
  ClickHouse enforces it itself: a query or merge that would go over it
  fails, and the engine stays up. There is no separate kernel-enforced
  limit.
- **CPU.** The CPU cap pins the engine to whole CPUs with
  `sched_setaffinity`. A fractional cap rounds up: `1.5` pins the engine to
  2 CPUs, and a cap never exceeds the CPUs Forge may use. The worker starts
  the engine from a short-lived thread that pins itself first, so the
  engine is pinned from its first instruction and no thread of Forge's own
  processes is ever pinned. A change pins every thread of the running engine at once. The
  engine's `max_threads`, `concurrent_threads_soft_limit_num` and
  background pool sizes follow the cap; they resize when the engine
  restarts.

CPU and memory usage on the status panel come from `/proc/<pid>/stat` and
`/proc/<pid>/status` of the engine process. The embedded engine runs on
Linux only; on Windows, use an external ClickHouse.

For an external ClickHouse (`engine = "clickhouse"`) Forge does not manage
CPU or memory: the API refuses changes to these settings, and the console
hides them. Retention, search and ingest settings stay editable.

Shortening retention makes older data eligible for deletion, so it needs
an explicit confirmation. Restarting the engine never restarts `forge-server`: node history and
log pages report analytics as unavailable for a few seconds, and ingest
answers `503` until the engine is back.

The search and ingest limits protect the host from its own users. A cap on
concurrent searches, a per-search timeout, threads and memory, and a
maximum lookback keep a wide log search from starving ingest. A bounded
ingest queue and a free-disk floor make senders back off instead of filling
memory or the disk.

## Rules between the two

- **No shared transactions or joins.** Analytics rows refer to operational
  records by id only (`server_id`, `application_id`). Names are looked up in
  the operational database when a page is shown.
- **Operations never wait for analytics.** When the analytics database is
  down or full, deployments, access checks and jobs carry on.
- **The audit log stays operational.** It is security evidence with a
  tamper-evident chain.
- **One name each.** On a PostgreSQL server that holds both, they are two
  separate databases, never one.

## Choosing engines

| Fleet | Operational | Analytics |
| --- | --- | --- |
| Lab, or up to about 50 nodes with modest logging | SQLite | DuckDB (the default) |
| A team that runs PostgreSQL | PostgreSQL | DuckDB or Embedded |
| Heavy logging, or 100 nodes and more | PostgreSQL | Embedded, or a ClickHouse server |

**Which analytics store.** All three stay supported.

- **DuckDB** (`duckdb`, the default) runs inside the analytics worker. There is
  nothing to download, it is idle when nothing happens, and it uses the least
  memory. It scans every row of a time window to search text, so wide log
  searches get slow on large volumes.
- **Embedded** (`embedded`, chDB) is ClickHouse run by Forge. It costs about 15 %
  of a core and about 600 MB (with its mapped binary) when idle. Its token
  index keeps text searches fast on large volumes, and it uses the same SQL
  as a ClickHouse server.
- **ClickHouse** (`clickhouse`) is a server you run, for heavy load and many
  nodes.

Switching engines starts with empty history; Forge does not copy rows
between them.

## Related

- [Choose and connect databases](/docs/guides/databases/)
- [Send and search service logs](/docs/guides/application-logs/)
- [Configure analytics retention and limits](/docs/guides/analytics-settings/)
- [Configuration: databases](/docs/reference/configuration/#databases)
- [Spool file format](/docs/reference/spool-file-format/)
- [Network ports](/docs/reference/network-ports/)
