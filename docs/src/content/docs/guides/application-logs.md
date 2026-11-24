---
title: Send and search service logs
description: Create a log ingest key for a service, send structured logs through the node's local listener (FORGE_LOG_URL) or to the server from Serilog, Python, Node.js or curl, and search them in the Logs console.
sidebar:
  order: 25
---

This guide shows you how to send structured logs from your services to Forge
and read them in the **Logs** console. Forge accepts the Seq ingestion
protocol, so loggers that write to Seq write to Forge without code changes.

A service that Forge runs on a node sends its logs to a listener on that
node. The node keeps them in its spool and uploads them to the analytics
worker. An app outside the fleet sends them to the Forge server. For how
this works, see [Analytics: node spools](/docs/concepts/analytics/#node-spools).

## Before you begin

- The server has an analytics database (`[analytics] engine` is `embedded`
  or `clickhouse`). See [Choose and connect databases](/docs/guides/databases/).
  With analytics off, the **Logs** page says
  **Log analytics is not available**, nodes run no log listener, and the
  server answers `503`.
- To create or revoke ingest keys you need `log.manage` on the service:
  `platform_admin`, `workspace_admin`, `operator` or `developer`.
- To set the service's environment variables you need `deploy` on its
  environment.
- To read logs you need `log.read` on the service: the roles above, plus
  `dba` and `auditor`. The `customer` role cannot read logs.
- The service exists in Forge. See [Services](/docs/guides/services/).
- For a service on a node: the node reaches the control plane on TCP
  `3832` (the analytics worker). See [Network ports](/docs/reference/network-ports/).
  Systemd and container services are supported on Linux nodes.
- For an app outside the fleet: the machine reaches the Forge server over
  HTTPS.

## Steps

### 1. Create an ingest key

1. Open the service's page.
2. In the **Log ingest keys** panel, select **New key** (or **Create a key**
   when the service has none).
3. Enter a **Name** that says where the key is used, for example
   `production`. It is 1 to 80 characters.
4. Select **Create key**.
5. Copy the key. It starts with `flk_`.

:::caution
Forge shows the key once. It stores only a SHA-256 hash and cannot show it
again. Anyone with the key can write events to this service, and to no
other. Keep it in your secret store and pass it to the service as an
environment variable, not in source code.
:::

### 2. Give the key and the URL to the service

**A service Forge runs on a node.** Forge already sets `FORGE_LOG_URL` in
the service's environment:

| Runtime | `FORGE_LOG_URL` |
| --- | --- |
| Systemd (and Forge's supervisor) | `http://127.0.0.1:5341` |
| Container | `http://forge-node:5341`. Forge adds `--add-host forge-node:host-gateway`, so `forge-node` is the node's Docker bridge address |
| Compose stack service | `http://forge-node:5341` in `environment`, and `forge-node:host-gateway` in `extra_hosts`. With `network_mode: host`, `http://127.0.0.1:5341` and no host entry. A service with `network_mode: service:<name>` or `container:<name>` gets only the URL; `network_mode: none` gets neither |

If you set `FORGE_LOG_URL` on the service yourself, your value wins. In a
compose stack, set it under the service's `environment`; a value in an
`env_file` is overridden by the one Forge adds. A `forge-node` entry you
put in `extra_hosts` is kept.

Add the key:

1. On the service's page, open **Environment variables**.
2. Add `FORGE_LOG_KEY` with the key as its value. Forge marks it as a
   secret because the name ends in `_KEY`.
3. Save and deploy the service.

The node accepts a key only for services placed on that node. It learns
new keys with its next heartbeat, within a few seconds.

**An app outside the fleet.** Use the Forge server's base URL, for example
`https://<forge-server>`, and pass the key in the same variables:

```sh
export FORGE_LOG_URL='https://<forge-server>'
export FORGE_LOG_KEY='<token>'
```

### 3. Configure your logger

Point the logger at `FORGE_LOG_URL`. Do not add a path: the clients add
`/api/events/raw` themselves. The examples fall back to the server URL so
the same code runs off the fleet.

The node's listener answers `429` while its spool is full or, for
containers, when a key sends more than 50 requests a second. Use a client
that batches and retries, such as the sinks below. Send batches, not one
request per event.

#### .NET (Serilog)

Install `Serilog.Sinks.Seq` and add the sink:

```csharp
using Serilog;

var url = Environment.GetEnvironmentVariable("FORGE_LOG_URL") ?? "https://<forge-server>";

Log.Logger = new LoggerConfiguration()
    .Enrich.FromLogContext()
    .WriteTo.Seq(url,
        apiKey: Environment.GetEnvironmentVariable("FORGE_LOG_KEY"))
    .CreateLogger();

Log.Information("Order {OrderId} placed by {Customer}", 1042, "acme");
```

The sink batches events and retries a refused batch. To keep events on
disk while the target is unreachable, pass
`bufferBaseFilename: "logs/forge-buffer"` to `WriteTo.Seq`.

#### Python (`seqlog`)

```sh
pip install seqlog
```

```python
import logging
import os

import seqlog

seqlog.log_to_seq(
    server_url=os.environ.get("FORGE_LOG_URL", "https://<forge-server>") + "/",
    api_key=os.environ["FORGE_LOG_KEY"],
    level=logging.INFO,
    batch_size=50,
    auto_flush_timeout=5,
    override_root_logger=True,
)

logging.info("Order {OrderId} placed by {Customer}", OrderId=1042, Customer="acme")
```

#### Node.js (`pino-seq` or `seq-logging`)

```sh
npm install pino pino-seq
```

```js
const pino = require("pino");
const pinoSeq = require("pino-seq");

const stream = pinoSeq.createStream({
  serverUrl: process.env.FORGE_LOG_URL || "https://<forge-server>",
  apiKey: process.env.FORGE_LOG_KEY,
});
const log = pino({ level: "info" }, stream);

log.info({ OrderId: 1042, Customer: "acme" }, "Order placed");
```

With `seq-logging` directly, pass the same `serverUrl` and `apiKey` to
`new Logger({ serverUrl, apiKey, onError: console.error })`.

#### Any language (CLEF over HTTP)

Send newline-delimited JSON in the Compact Log Event Format (CLEF):

```sh
curl -sS -X POST "${FORGE_LOG_URL:-https://<forge-server>}/api/events/raw?clef" \
  -H "X-Seq-ApiKey: $FORGE_LOG_KEY" \
  -H "Content-Type: application/vnd.serilog.clef" \
  --data-binary @- <<'EOF'
{"@t":"2026-01-02T03:04:05.678Z","@l":"Warning","@mt":"Disk {Mount} at {Percent}%","Mount":"/var","Percent":91}
{"@t":"2026-01-02T03:04:06.000Z","@mt":"Job {JobId} done","JobId":"j-17"}
EOF
```

An accepted batch answers `201` with `{"MinimumLevelAccepted":null}`. The
node's listener answers `201` once the body is in its spool; it does not
parse the events, so a malformed line is skipped later, when the worker
stores the file. The server parses the batch first and refuses a malformed
one with `400`. See
[Service logs in the REST API](/docs/reference/rest-api/#service-logs)
for the fields Forge reads and the limits.

### 4. Read the logs

1. Go to **Operations → Logs**, or select **Open log stream** on the
   service's page to open it filtered to that service.
2. Open the **Service** filter, or keep **All services** to see every
   service you may read. Tick one or more services. For a service that runs
   on two or more nodes, expand it and tick single nodes to see only what
   those nodes sent. Ticking a whole service also shows its events whose
   node is unknown. The search box at the top filters services and node
   names. Use the arrow keys to move, Space to tick, and Right or Left to
   expand or collapse.
3. Narrow the stream with the second filter row:
   - **Source**: **Sent by services** (ingested events) or **Container
     output** (lines the nodes collect from Docker, see
     [Container log collection](/docs/concepts/container-logs/)).
   - **Node** (several), **Container** and **Stack**: the options come from
     the loaded events.
   - **Clear filters** resets them.

   The filters are kept in the URL: `/logs?source=container&node=<node-id>&container=<container-name>&stack=<stack>`.
   `node` takes comma-separated node ids. Container lines that belong to no
   service need `view` on their node.
4. Select levels in the level group to show only those levels.
5. Type in the search box and press Enter or select **Run**. **Run** with
   an unchanged or empty search loads the logs again:
   - Free text matches the message and the exception, ignoring case.
   - `Name="value"` or `Name=value` matches a property exactly, for example
     `Customer="acme" timeout`. Up to 8 property filters apply; free text is
     cut at 500 characters.
6. Each row starts with a source badge, **app** (sent by a service) or
   **ctr** (container output), then the service, or the container name for
   a line without a service (without its stack prefix), then `@` and the
   node; **err** marks a stderr line. A long message shows its first two
   lines. Select a row to expand it. It shows the full message, the node, the **Source** (`ingest` or
   `container · stdout|stderr`), the **Container** (a link to its page), the
   **Container id** and the **Stack**, the properties, the exception, the
   trace and span ids and when Forge ingested the event. Select **Find**
   next to a property to add it to the search.
7. Scroll down to load older events, or select **Load more**.
8. Select **Live**, next to **Run**, to show new events as they arrive.
   The page asks for new events every 2 seconds. Select **Live** again, or
   **Run**, to stop.

The histogram above the stream counts events per level over the window
(the last 24 hours by default). Select **Chart** to hide or show it; the
browser remembers your choice.

## Verify

1. On the node, as the service's user, send one event with the curl
   example in step 3 and the service's `FORGE_LOG_URL` and `FORGE_LOG_KEY`.
   For a container, run it inside the container
   (`docker exec -it forge-app-<environment>-<service> sh`).
2. Check that the response is `201`.
3. Open **Logs**, pick the service and find the event at the top of the
   stream. From a node it appears after the spool seals and uploads the
   file, about 10 seconds by default.
4. Off the fleet, the key's last use time on the service page is set.
   Forge updates it at most once a minute.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `401 invalid_api_key` (server) or `401 missing or unknown API key` (node) | The `X-Seq-ApiKey` header is missing, wrong or the key was revoked | Check the variable that holds the key; create a new key if it was revoked |
| `401` from the node with a valid key | The key belongs to a service that is not placed on this node, or the node has not had a heartbeat since the key was created | Send with the key of a service on this node; wait a few seconds after creating a key |
| `Connection refused` on `127.0.0.1:5341` | The node runs no listener: analytics is off on the server, or the agent is from an older release | Turn on analytics; [update the agent](/docs/guides/update-agents/) |
| `Could not resolve host: forge-node` in a container | The container was not deployed by Forge as a service or stack, or its stack was deployed before Forge added the entry | Redeploy the service or stack, or add `forge-node:host-gateway` to the container's extra hosts |
| `Connection refused` from `forge-node:5341`, and the agent logged that the Docker bridge gateway is not on this host | The node runs in a container and uses the host's Docker socket, so the bridge address is not its own | Run the app on the node itself and use `http://127.0.0.1:5341`, or send to the server URL |
| `429 the node's log spool is full` | The node cannot upload (TCP `3832` blocked or the worker down) and the spool reached its cap, or the node's disk is below the free-space floor (256 MiB) | Open TCP `3832` from the node to the control plane; free disk on the node. Clients that retry resend the batch |
| `429 rate limited` | A container sends more than 50 requests a second with one key | Send larger batches less often; a platform administrator can raise `[analytics.listener] rate_per_sec` |
| `413 body too large` from the node | A request body is over 1 MiB | Lower the client's batch size |
| Events from a node never appear, the node answers `201` | The node cannot reach the worker on TCP `3832`, or the lines are not valid CLEF | Open the port; check the agent's log for upload errors; send one JSON object per line |
| `400 line N is not a JSON object` (server) | A CLEF line is not one JSON object | Send one JSON object per line, with no wrapping array |
| `400 a batch holds at most 10000 events` | The request holds more than 10,000 events | Lower the client's batch size |
| `503 analytics_off` | `[analytics] engine = "off"` | Turn on analytics; see [Configuration](/docs/reference/configuration/#analytics) |
| `503 analytics_unavailable` | The analytics database cannot be reached | Check the database; clients that retry (Serilog's durable sink) resend the batch |
| `503 ingest_busy` | More ingest requests arrive than the analytics database can store: over 4,096 requests, or more than the queued-bytes limit (default 256 MiB), are waiting | Send larger batches less often (Serilog's `batchPostingLimit`, `period`); clients that retry resend the batch. A platform administrator can raise the limit; see [Configure analytics retention and limits](/docs/guides/analytics-settings/) |
| `503 ingest_disk_low` | The analytics engine has less free disk than the minimum (default 1024 MiB) | Free disk or shorten retention; clients that retry resend the batch |
| `503 search_busy` on a search | Too many log searches run at once (default 8) | Retry shortly; a platform administrator can raise the limit |
| A search shows no events older than some days | A maximum search lookback is set | Ask a platform administrator; see [Configure analytics retention and limits](/docs/guides/analytics-settings/) |
| Events show the wrong time | `@t` comes from the app's clock | Fix the clock on the node; retention still counts from the ingest time |
| A long message ends in `…` | Messages, templates and exceptions are cut at 32 KiB | Log less per event, or move large data out of the message |
| The event is in **Information** though the app logged another level | Forge did not recognise the level name | Use a standard name such as `Warning`, `warn` or `error` |
| A node filter shows no events, but the whole service does | The app sends to the server URL, so Forge cannot match the event to a node | Send to `FORGE_LOG_URL` on the node, or add a `MachineName` property (Serilog `Enrich.WithMachineName()`) |
| **No log events yet** | No event arrived for the services you may read | Check the key, the URL and that the client flushed |

## Next steps

- [Analytics and the two databases](/docs/concepts/analytics/#how-service-logs-are-stored)
  explains how logs are stored and kept.
- [REST API: service logs](/docs/reference/rest-api/#service-logs) lists
  every endpoint and field.
- [Configuration: `[analytics.listener]`](/docs/reference/configuration/#analyticslistener)
  lists the node listener's limits.
- [Cedar schema](/docs/reference/cedar-schema/#actions) lists `log.read` and
  `log.manage` per role.
