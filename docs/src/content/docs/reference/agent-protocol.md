---
title: Agent protocol
description: forge.agent.v1 messages, endpoints and verification rules, including the analytics spool upload to the worker.
sidebar:
  order: 6
---

HTTP/2 over TLS 1.2+/1.3, ALPN `h2`, `Content-Type: application/x-protobuf`.
Definitions: `src/protocol/forge-proto/proto/forge_agent_v1.proto` (the Rust
types in `messages.rs` are wire-compatible).

| Endpoint | Client cert | Request → Response |
| --- | --- | --- |
| `GET /agent/v1/ca` | no | — → CA PEM |
| `POST /agent/v1/enroll` | no | `EnrollRequest` → `EnrollResponse` |
| `POST /agent/v1/heartbeat` | **yes** | `Heartbeat` → `HeartbeatResponse` |
| `POST /agent/v1/renew` | **yes** | `RenewRequest` → `RenewResponse` |
| `POST /agent/v1/spool` (analytics worker, port `3832`) | **yes** | One sealed spool file → `SpoolAck`. See [Analytics spool](#analytics-spool) |

## SignedCommand

```
SignedCommand { body: bytes = encode(CommandBody), signature: Ed25519(body), key_id }
CommandBody   { operation_id, server_id, issued_at_ms, expires_at_ms, nonce, principal,
                oneof command { Ping | CollectFacts | ServiceStatus{name} | … | FileAction{op_json} } }
```

`FileAction` (tag 22, allowlist name `files`) carries one
`forge_core::files::FileOp` as JSON: `list`, `stat` or `read`, each with a
`root` and a public `path`. The agent validates it again, confines it to the
root and applies the protected-path policy before it opens anything. See
[File explorer](/docs/concepts/file-explorer/).

Tag 21 (`StorageAction`, allowlist name `storage`, object-store operations)
is retired and never reused; an agent that receives it fails the command as
unknown.

Tag 23 (`AccessAction`, WireGuard remote-access gateways) is retired and
never reused; an agent that receives it fails the command as unknown.

`TunnelOpen` (tag 38, allowlist name `tunnel`) opens one
[port tunnel](/docs/concepts/port-tunnels/) connection and runs in the agent
itself (no root). Fields: `connection_id` (32 hex), `port`, `allowed_ports`
(the signed copy of what this tunnel may reach: its one port), `stream_token`
(64 hex, single use; the server keeps its hash), `idle_secs`, `expires_at_ms`
and `dial_ip`. The agent refuses a port outside `allowed_ports`, a malformed
id or token, an expired command, and a `dial_ip` that is neither loopback
nor one of the node's own interface addresses (its cluster network address,
for example): a tunnel never pivots to another host. An empty `dial_ip`
means loopback (`127.0.0.1`, then `[::1]`); agents from before the field
ignore it and dial loopback. It then opens its own mTLS HTTP/2 connection to
the hub and posts a streaming request to `/agent/v1/tunnel/<connection_id>`
with the token in `x-forge-tunnel-token`; the request body carries bytes
from the node, the response body bytes to it.

`StackAction` (tag 31, allowlist name `stack`) carries one
`forge_catalog::stacks::StackOp` as JSON for a compose stack. The agent
validates it again and runs it in the helper, in the background; it refuses
it on Windows. The helper runs `docker compose -p <project>
--project-directory <dir> -f <dir>/compose.yaml …` with a cleared
environment.

| `op` | Fields | Does |
| --- | --- | --- |
| `deploy` | `part`, `services`, `pull` | Checks the part against the policy again (bind sources after resolving symlinks), writes `compose.yaml` (interpolated, `$` escaped) and `.env` with mode `0600`, then `up --detach --no-build --remove-orphans --wait`. Returns `{services, ports}` |
| `ps` | `project`, `dir` | `{services: [{service, name, state, health, status, image, exit_code, ports}]}` |
| `service` | `project`, `dir`, `action`, `services` | `start`, `stop`, `restart` or `pull`, then `ps` |
| `down` | `project`, `dir`, `volumes` | `down --remove-orphans` (`--volumes` when set), then removes `compose.yaml` and `.env` |
| `logs` | `project`, `dir`, `service`, `tail` | `{logs}`, at most 512 KiB |

`part` is `{environment, stack, dir, compose, env, policy: {allow_privileged,
bind_roots, stack_dir}, internal_ip}`. Agents enrolled before this command
existed get `stack` added to their allowlist when the node installer runs
again. See [Compose stacks](/docs/concepts/compose-stacks/).

`CancelAction` (tag 25, allowlist name `cancel`) carries the `operation_id`
of a background operation the agent is still running, such as a recipe
install. The agent drops the operation, the helper kills its
process group, and the operation's result is a failure with the error
`cancelled`. The cancel command
itself returns `{"cancelled": true}`, or `false` when nothing with that id was
running. The server sends it when a job is cancelled.

Verification order on the agent: signature → decode → target → TTL ≤ 300 s →
not expired → not in the future → issued after agent start → replay cache →
local allowlist. Nothing in the body is trusted before the signature passes.

## Container telemetry

Agents that spool (they set `Heartbeat.spool`) write container lines and
samples to the spool and leave the fields below empty; the server then
forwards nothing. The fields stay for agents of the previous release.

| Message | Field | Meaning |
| --- | --- | --- |
| `Heartbeat` | `container_logs` (9) | Old agents only. `ContainerLogBatch {seq, lines, dropped}`: up to 2,000 `ContainerLogLine {container_id, container_name, image, labels, stream, ts_ms, line}` (a line at most 16 KiB). Re-sent until acknowledged; `dropped` counts lines the node dropped since the last batch |
| `NodeMetrics` | `containers` (16) | Old agents only. Up to 256 `ContainerSample {container_id, container_name, image, labels, cpu_percent, mem_used_bytes, mem_limit_bytes, net_rx_bytes_per_sec, net_tx_bytes_per_sec, block_read_bytes_per_sec, block_write_bytes_per_sec, restarts}`, once a minute; empty in the heartbeats in between |
| `HeartbeatResponse` | `container_telemetry` (4) | `ContainerTelemetryConfig {enabled, exclude}`: what the node collects (at most 64 excludes). Applies to every agent |
| `HeartbeatResponse` | `container_logs_ack` (5) | The `seq` of the batch the server stored; `0` for none, and always `0` for a spooling agent |

`ContainerLabels` carries `forge_app` (`forge.app`), `forge_stack`,
`compose_project` and `compose_service`. Lines and samples are attributed
with the node taken from the client certificate. See
[Container log collection](/docs/concepts/container-logs/).

## Analytics spool

Every agent keeps a disk spool of node history, top processes, container
output and samples, and the application logs its local listener receives.
It uploads the spool straight to the analytics worker, not through the
agent hub. Heartbeats then carry only the live sample, the spool status and
the settings. See [Analytics: node spools](/docs/concepts/analytics/#node-spools).

### Fields

| Message | Field | Meaning |
| --- | --- | --- |
| `EnrollResponse` | `analytics_url` (6) | The worker's upload URL (`https://host:3832`); empty while analytics is off. The agent stores it in `agent.toml` |
| `EnrollResponse` | `analytics_port` (8) | The worker's upload port. A new agent dials it on the host of its own `server_url`; `0` means use `analytics_url` (analytics off, or `[analytics] worker_agent_url` is set) |
| `Heartbeat` | `spool` (10) | `SpoolStatus`, present from agents that spool. Its presence tells the server not to expect `container_logs`, `NodeMetrics.containers` or `top_processes` |
| `HeartbeatResponse` | `analytics` (6) | `AnalyticsAgentConfig`; absent while analytics is off, and the agent then writes nothing to the spool |

| Message | Field | Type | Meaning |
| --- | --- | --- | --- |
| `AnalyticsAgentConfig` | `url` (1) | string | `https://host:3832` of the worker (`[analytics] worker_agent_url`) |
| | `spool` (2) | `SpoolTuning` | Spool limits |
| | `keys` (3) | repeated `IngestKey` | Ingest keys of the services placed on this node, at most 4,096 |
| | `listener` (4) | `ListenerTuning` | Local listener limits |
| | `port` (5) | uint32 | The upload port on the host of the agent's own `server_url`; `0`: use `url`. Older agents use `url` |
| `IngestKey` | `hash` (1) | bytes | SHA-256 of the key |
| | `service_id` (2) | string | The key's service |
| `SpoolTuning` | `max_bytes` (1) | uint64, bytes | Spool cap |
| | `min_free_bytes` (2) | uint64, bytes | Free-disk floor |
| | `max_age_secs` (3) | uint64, seconds | Oldest file kept |
| | `seal_bytes` (4) | uint64, bytes | Seal size |
| | `seal_secs` (5) | uint64, seconds | Seal interval |
| | `zstd_level` (6) | uint32 | 1 to 3 |
| `ListenerTuning` | `max_body_bytes` (1) | uint64, bytes | Largest request body |
| | `rate_per_sec` (2) | uint32 | Requests per second per key |
| | `rate_on_loopback` (3) | bool | Rate loopback requests too |
| | `extra_address` (4) | string | An extra address to listen on (reserved for a mesh address); never a wildcard. The server sends it empty |
| `SpoolStatus` | `files` (1) | uint64 | Sealed files waiting |
| | `bytes` (2) | uint64 | Bytes they hold |
| | `dropped_files` (3) | uint64 | Files dropped at the cap or for age |
| | `dropped_bytes` (4) | uint64 | Bytes of those files |
| | `oldest_age_secs` (5) | uint64 | Age of the oldest waiting file |
| | `uploaded_files` (6) | uint64 | Files the worker acknowledged |
| | `refused_requests` (7) | uint64 | Listener requests answered `429` because the spool was full or the disk below the floor |
| `SpoolAck` | `file_id` (1) | string | The stem of the stored file |
| | `rows` (2) | uint64 | Rows stored |
| | `duplicate` (3) | bool | The file was stored before; nothing was written again |

In `SpoolTuning` and `ListenerTuning`, `0` keeps the agent's default; see
[Configuration: `[analytics.spool]`](/docs/reference/configuration/#analyticsspool).

### Upload

```http
POST /agent/v1/spool HTTP/2
Host: <forge-host>:3832
Content-Type: application/x-ndjson+zstd
x-forge-spool-file: <node-id>-<file-id>

<the sealed file, as written>
```

HTTP/2 over TLS with the agent's client certificate, verified against the
Forge CA like the agent hub. The body is one file in the
[spool file format](/docs/reference/spool-file-format/). The agent sends
its files oldest first, one at a time, and deletes a file only after a
`200` whose `SpoolAck.file_id` names it.

| Status | Body | Meaning | Agent does |
| --- | --- | --- | --- |
| `200` | `SpoolAck` | Stored, or stored before (`duplicate`) | Deletes the file, sends the next |
| `400` | text | `x-forge-spool-file` missing or malformed, or the body could not be read | Drops the file and logs a warning |
| `403` | text | Not an agent certificate, the node is not active, the certificate is not the node's current one (replaced at renewal or revoked), or the file names another node | Retries with back-off |
| `404`, `405` | text | Wrong path or method | Retries with back-off |
| `413` | text | Larger than 16 MiB | Drops the file and logs a warning |
| `422` | text | The file is not a readable spool file (bad zstd, header or row) | Drops the file and logs a warning |
| `429` | text | `worker_max_uploads` imports already run, or the worker's ingest queue is full | Backs off with jitter (1 s to 60 s) and retries |
| `503` | text | The analytics database cannot take the rows, or analytics is unavailable | Backs off and retries |

The worker remembers the last 64 file ids per node across restarts
(`<data_dir>/analytics-worker/spool-ids/seen.log`), so a file resent after
a lost acknowledgement is answered `duplicate` and not stored twice.

## Self-update capability

`CollectFacts` answers include `self_update`, which the root helper
computes on the node (`forge_proto::facts::SelfUpdateCapability`). Older
agents leave it out, and the control plane then treats the node as
`unknown`.

| Field | Type | Notes |
| --- | --- | --- |
| `supervisor` | string | `systemd`, `respawn`, `none` or `unknown` |
| `ready` | bool | `true` when `reasons` is empty |
| `reasons` | array of string | Blockers: `no_systemd`, `no_curl`, `bin_dir_not_writable` (`/usr/local/bin` read-only or not writable), `low_disk` (less than twice the binaries' size free there) |
| `warnings` | array of string | `ephemeral_container`: the binaries sit in a container's image layer, so an update lasts until the container is recreated |
| `helper_version` | string | The helper's version |

Linux agents also report `helper_ok`: `true` when the helper answered the
capability question. After an update, `false` fails the update job, because
a node without a working helper has no rollback watchdog.

Unknown values from a newer agent read as `unknown`. The control plane
re-collects facts when a node reports another agent version.

## Command results

The agent answers each command with a `CommandResult`:

| Tag | Field | Type | Notes |
| --- | --- | --- | --- |
| 1 | `operation_id` | string | The command's operation id |
| 2 | `ok` | bool | `true` when the command succeeded |
| 3 | `output` | string | The result, usually JSON |
| 4 | `error` | string | Why it failed |
| 5 | `error_code` | string | A refusal the control plane acts on, or empty. Older agents leave it empty |

`AgentUpdate` carries `version`, `download_base`, `arch`, `agent_sha256`,
`helper_sha256` and `allow_downgrade` (tag 6, default `false`). The helper
refuses a version older than its own unless `allow_downgrade` is set; the
control plane never sets it today. Older helpers ignore the field.

`error_code` values, today sent for `AgentUpdate` only:

| Value | Meaning | What the control plane does |
| --- | --- | --- |
| `unsupported` | The node cannot self-update: not Linux, no systemd, no `curl`, or `update` is not in `allowed_commands` | A rollout skips the node; a single update fails |
| `busy` | Another update holds the node's update lock | The update job retries with backoff |
| `downgrade` | The command would install an older version | A rollout skips the node; a single update fails |

## Server key pinning

Agents accept the control plane by the key in its certificate, not by host
name. See [Pinned server key](/docs/concepts/security-model/#pinned-server-key).

| Message | Field | Type | Meaning |
| --- | --- | --- | --- |
| `EnrollResponse` | `server_keys` (7) | `SignedServerKeys` | The keys to pin at enrollment; absent from older servers |
| `Heartbeat` | `pinned_server_keys` (11) | repeated string | Fingerprints the agent pins: its acknowledgement of an announced next key. Empty from agents without pins |
| `HeartbeatResponse` | `server_keys` (7) | `SignedServerKeys` | The current and next keys, in every reply |
| `SignedServerKeys` | `body` (1) | bytes | An encoded `ServerKeys` |
| | `signature` (2) | bytes | Ed25519 over `body` by the command key |
| `ServerKeys` | `server_id` (1) | string | The node the announcement is for |
| | `issued_at_ms` (2) | int64 | When it was signed |
| | `keys` (3) | repeated `ServerKey` | At most 4 |
| `ServerKey` | `sha256` (1) | string | SHA-256 of the SubjectPublicKeyInfo, 64 lower-case hex digits |
| | `label` (2) | string | `current` or `next` |

The agent's verifier accepts a server certificate when the SHA-256 of its
SubjectPublicKeyInfo is in the pin set, the certificate is within its validity
period and it allows server authentication. On Linux the root helper checks the
command-key signature and that the announcement names this node before it
writes the pin file. An agent with no pins (enrolled before pinning, against an
older server) keeps checking the Forge CA and the host name.

## Local names

The server sends a signed `HostNames` command with a node's
[local names](/docs/concepts/local-names/). The agent sends its compose
containers back so names follow replicas without a deploy.

| Message | Field | Type | Meaning |
| --- | --- | --- | --- |
| `HostNames` | `entries_json` (1) | string | JSON array of `{"name", "ip"}` for the hosts-file block. Sent to every node |
| | `containers` (2) | bool | Also write the block into the node's Forge container hosts files |
| | `suffix` (3) | string | The suffix every name ends in. The node refuses names outside it |
| | `records_json` (4) | string | JSON `Records` for forge-dns (A, SRV, PTR, TTL): the node's cluster set. Empty outside a fabric and from older servers; the agent then writes no `names.json` |
| | `zone` (5) | string | The zone forge-dns answers for, `<cluster>.<suffix>`. The node also derives the containers' `dns_search` from it |
| | `upstream_forge_dns` (6) | repeated string | The IPs of the server host's forge-dns (`[dns] listen`); may be empty |
| `Heartbeat` | `stack_states` (12) | `StackStates` | The node's compose containers. Sent only when they changed, at most every 10 seconds. Absent from older agents |
| `Heartbeat` | `app_states` (13) | `AppStates` | The node's native service containers (label `forge.app`) as a JSON array of `{app, slot, release, state, exit_code, container_id}`. Sent only when they changed, at most every 10 seconds. Agents that send it report the fact `app_states: true`. Absent from older agents |
| `StackStates` | `states_json` (1) | string | JSON object: stack label (`<environment>/<stack>`) → rows `{service, name, number, state, health, mesh_ip}` |

The agent reads its containers through the helper on every heartbeat. That
request is read-only and unsigned. The server merges the rows by container
name into each stack node's stored state (`state`, `health`, `number`,
`mesh_ip`, `service`), keeps `ports` and `image` from the last deploy, drops
containers that vanished, and rebuilds the names. See
[forge-dns](/docs/reference/forge-dns/) for the `names.json` format.

## Long-poll

When no command is pending, the hub holds the heartbeat for
`heartbeat_hold_secs` or until a command is queued. `next_interval_secs = 0`
tells the agent to reconnect immediately; results of executed commands are
sent on the next heartbeat straight away.

## Secret and config files

An `app` command's `spec_json` (an `AppSpec`) may carry `files`: one entry
per secret or config file, `{name, target, mode, uid, gid, value}` (D-179).
The server fills it only for the `deploy` action, inside the job step that
unseals the credential vault items, and only for agents whose facts report
`"service_files": true`; for any other agent the step fails with "update the
agent first". `value` exists only inside the signed command: `AppSpec` and
`MountedFile` print it as `***` in debug output, and neither side logs it.

The helper writes each file under
`/var/lib/forge-agent/secrets/<environment>/<service>[--<slot>]/` (parents
`0711`, the folder root-owned `0700`, or `root:<account group>` `0750` for a
native service), to a fresh temporary file opened without following links,
sets owner and mode, renames it into place and removes files no longer
listed. A container gets each file bind-mounted read-only at `target`; a
native service gets the folder in `FORGE_SECRETS_DIR`. `remove` and `retire`
delete the folder.

## Compatibility

Fields are only ever added with new tags. New command variants are ignored
(rejected as malformed) by older agents, which report no result; the job then
retries and fails with a timeout, visible to the operator.
