# chDB uncapped, 50 nodes with one real forge-agent

[← All benchmarks](README.md)

| | |
| --- | --- |
| Date | 2026-11-22 |
| Commit | `1a5efe8` |
| System | 16 vCPU Xeon E5-2620 v2 (no AVX2), 62 GB RAM, local NVMe, Linux 6.8; Docker for the real node's 20 containers |
| Flow | Real agent + root helper (Docker log collection) and 49 simulated nodes → worker → embedded chDB |
| Store | chDB, uncapped |
| Load | Same as [50 × 20 uncapped](2026-11-22-chdb-uncapped-50x20.md), plus a 60 s upload outage |
| Verdict | Agent: **0.6 % of one core, 20 MB** (helper adds 0.5 %, 17.6 MB). Exactly once through the outage; spool drained in 15 s |

Measures a real `forge-agent` on a node with real containers, and what
happens to its spool when the worker cannot be reached.

| Item | Value |
| --- | --- |
| Engine | Uncapped, verified: `Cpus_allowed_list: 0-15`, `max_server_memory_usage` 53.2 GiB, per-query 1 TiB |
| `worker_max_uploads` | 8 (default) |
| Simulated | 49 fleet-sim nodes: `run_phase.sh <name> 49 <secs> 100 4 20 300 10` |
| Real node | `forge-agent run` (as the user) and `forge-agent-helper serve --agent-dir <dir>` (as root), enrolled with `forge-server enroll-token` |
| Collection path | **Real Docker log collection.** The helper follows the containers on the Docker socket; the agent spools and uploads to the worker. The local HTTP listener was not used |
| Real node's load | 20 `alpine/socat` containers started with `sg docker` (`containers.sh`): 10 print 10 lines/s to stdout, 10 are silent |
| Agent sampling | `agent_sample.py`: CPU, RSS, spool, bytes sent per connection from `ss` (the agent keeps one HTTP/2 connection per endpoint), every 5 s |
| Phases | `main`: 30 s idle, 4 min load, drain, 2 min after (sampled for about 7 min). `stop`: 3 min load; from 40 s to 100 s the worker's upload port is closed with `iptables -j REJECT --reject-with tcp-reset` |

The `stop` phase blocks the worker's port instead of stopping the worker.
The supervisor restarts a worker that misses 3 pings (about 15 s), so
SIGSTOP cannot hold it down for 60 s, and forge-server was not to be
killed. For the agents the effect is the same: every upload fails, and
the spools fill.

### Exactly once

| Phase | Source | Sent | Acked | Stored | Distinct |
| --- | --- | ---: | ---: | ---: | ---: |
| main | real agent | 24,000 | 24,000 | 24,000 | 24,000 |
| main | 49 simulated | 1,171,330 | 1,171,330 | 1,171,330 | (table) |
| stop | real agent | 18,000 | 18,000 | 18,000 | 18,000 |
| stop | 49 simulated | 860,885 | 860,885 | 860,885 | (table) |

The whole table holds 2,074,315 rows, all distinct (including 100 rows
from a smoke test). For the real agent, "sent" is `docker logs` lines.
"Acked" counts the agent's spool files: every sealed file is in the
worker's `seen.log`, none was dropped or refused, and the spool was empty
at the end. "Stored" counts rows of the agent's `server_id` from these
containers. No duplicates.

### Real agent

| | Idle | Load (avg) | Peak | After load | After the outage |
| --- | --- | --- | --- | --- | --- |
| Agent CPU (% of one core) | 0–0.2 | 0.6 | 6.9 (at seal and upload) | 0–0.4 | 0–0.4 |
| Agent RSS MB (anon + file) | 11.4 (2.8 + 8.7) | 19–20 | 20.1 | 20.1 (11.4 + 8.7) | 20.1 (11.5 + 8.7) |
| Helper CPU | 0 | 0.5 | 1.2 | 0–0.2 | 0–0.2 |
| Helper RSS MB (anon + file) | 8.3 (0.9 + 7.4) | 16 | 17.6 | 17.6 (10.1 + 7.4) | 17.6 |
| Spool | 0 files | 0 files (each file is uploaded within a second of sealing) | 3 files, 19.6 KB (during the outage) | 0 | 0 |
| Upload to the worker | – | 0.4 KB/s (98 KB for 24,000 lines, about 4 B per line) | 4.5 KB/s (the backlog after the outage) | – | – |
| Heartbeat link to forge-server | – | 7.9 KB per 12 min | – | – | – |

- **Memory.** RSS rose from 11.4 to 20.1 MB in the first minute of load
  and then stayed flat. It did not grow during the 60 s outage or the
  drain, and did not fall back after the load. That is allocator
  retention. The helper behaves the same way, 8.3 to 17.6 MB.
  Agent plus helper: about 38 MB.
- **Outage.** The agent's spool grew by one sealed file per heartbeat
  (1, 2, then 3 files, 19.6 KB). About 15 s after the port reopened it was
  empty. The 49 simulated nodes held 147 files at most. All spools
  drained, and stored rows matched sent rows.
- **File descriptors.** The agent held 1,100 to 1,550 fds, nearly all
  `/proc/<pid>/stat` handles that its process sampler keeps open for every
  host process. The count follows the number of host processes, not
  the load. It stays within systemd's default soft limit, but it is worth
  capping.

### Server side

| | main | stop (with outage) |
| --- | --- | --- |
| Offered / acked / stored rows/s | 5,465 / 5,467 / 4,692 | 5,668 / 5,573 / 6,185 |
| forge-server CPU avg / max, RSS max | 18 / 25 %, 66 MB | 18 / 27 %, 86 MB |
| Worker CPU avg / max, RSS max | 22 / 85 %, 79 MB | 21 / 86 %, 81 MB |
| chDB CPU avg / max, own MB max (+ 520 MB mapped binary) | 195 / 387 %, 352 MB | 348 / 519 %, 383 MB |
| chDB own MB after the load | 162 (2 min later) | 206–220 |
| Upload 429s / other deferrals (fleet-sim) | 953 / 232 | 388 / 378 (the outage) |
| Backlog max (simulated nodes), drain | 147 files, 10 s | 147 files, 10 s |
| Search p50 / p95 ms | 138 / 334 | 179 / 584 |
| Histogram p50 / p95 ms | 274 / 429 | 520 / 705 |
| Failed reads, server ERROR / WARN | 0, 0 / 0 | 0, 0 / 0 |
| Heartbeat p95 max | 135 ms | 97 ms |

### Over time

`main` phase, every 15 s. CPU is % of one core and memory is MB. The load
runs from 31 s to 271 s.

| t | Phase | Agent CPU | Agent RSS | Helper CPU | Helper RSS | Agent spool | Upload KB/s | Server CPU | Server RSS | Worker CPU | Worker RSS | chDB CPU | chDB own | Rows stored |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 16 | idle | 0.0 | 11.4 | 0.0 | 8.3 | 0 | 0.0 | 0.4 | 39 | 0.0 | 10 | 13 | 100 | 100 |
| 46 | load | 0.0 | 11.4 | 0.4 | 9.0 | 0 | 0.0 | 15.2 | 65 | 10.4 | 15 | 106 | 155 | 6,745 |
| 62 | load | 0.0 | 16.1 | 0.4 | 12.2 | 0 | 0.0 | 22.4 | 65 | 69.6 | 48 | 126 | 304 | 61,189 |
| 92 | load | 0.0 | 19.1 | 0.4 | 15.7 | 0 | 0.0 | 19.8 | 65 | 34.0 | 59 | 197 | 265 | 249,299 |
| 123 | load | 1.8 | 19.9 | 1.0 | 16.1 | 0 | 1.2 | 15.2 | 65 | 2.2 | 65 | 120 | 191 | 386,253 |
| 154 | load | 0.2 | 20.0 | 0.4 | 16.3 | 0 | 0.0 | 14.8 | 66 | 1.4 | 71 | 142 | 195 | 565,897 |
| 185 | load | 0.0 | 20.0 | 0.2 | 16.7 | 0 | 0.0 | 13.0 | 66 | 1.6 | 73 | 141 | 203 | 690,872 |
| 216 | load | 0.0 | 20.0 | 0.4 | 17.3 | 0 | 0.0 | 13.6 | 66 | 1.8 | 71 | 146 | 219 | 815,919 |
| 246 | load | 0.0 | 20.1 | 0.6 | 17.4 | 0 | 0.0 | 20.6 | 66 | 32.0 | 75 | 207 | 264 | 963,412 |
| 277 | after | 1.9 | 20.1 | 0.6 | 17.6 | 0 | 1.2 | 3.0 | 66 | 74.4 | 79 | 178 | 309 | 1,180,610 |
| 292 | after | 0.0 | 20.1 | 0.0 | 17.6 | 0 | 0.0 | 0.2 | 66 | 0.2 | 79 | 13 | 170 | 1,190,660 |
| 353 | after | 1.1 | 20.1 | 0.6 | 17.6 | 0 | 0.5 | 2.4 | 66 | 0.0 | 79 | 13 | 170 | 1,195,430 |
| 445 | after | 0.0 | 20.1 | 0.2 | 17.6 | 0 | 0.0 | 0.6 | 66 | 0.0 | 79 | 14 | 162 | 1,195,430 |
| 705 | after | 1.0 | 20.1 | 0.4 | 17.6 | 0 | 0.0 | 0.4 | 66 | 0.0 | 79 | 13 | 170 | 1,195,430 |

`stop` phase around the outage (port closed from 47 s to 107 s):

| t | Agent CPU | Agent RSS | Agent spool files / KB | Upload KB/s | Sim spool files | Worker CPU | chDB CPU | Rows stored |
| ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 31 | 6.9 | 20.1 | 0 / 0 | 1.4 | 4 | 120 | 345 | 1,311,430 |
| 61 | 0.0 | 20.1 | 1 / 5.8 | 0.0 | 49 | 1.8 | 275 | 1,321,456 |
| 92 | 0.2 | 20.1 | 2 / 12.0 | 0.0 | 98 | 4.2 | 308 | 1,321,456 |
| 107 | 6.1 | 20.1 | 3 / 19.6 | 0.0 | 147 | 1.8 | 307 | 1,321,456 |
| 123 | 0.0 | 20.1 | 0 / 0 | 0.0 | 104 | 29.6 | 355 | 1,435,293 |
| 153 | 2.1 | 20.1 | 0 / 0 | 2.3 | 24 | 86.4 | 519 | 1,885,440 |
| 200 | 0.0 | 20.1 | 0 / 0 | 0.0 | 0 | 0.0 | 13 | 2,070,315 |

chDB keeps working while the port is closed: those are merges and the
console's reads, and no rows arrive.

### Verdict for the agent

Against the plan's target (under about 30 MB RSS and a few percent of one
core):

- **CPU: met.** 0.6 % of one core on average at 100 lines/s from 20
  containers, with short peaks of 6–7 % when it seals and uploads. The
  helper adds 0.5 %.
- **RSS: met for the agent, not for agent plus helper.** The agent stays
  at 20 MB and does not grow under a backlog. The helper adds 17.6 MB,
  about 38 MB for the node's Forge processes. Both keep their peak
  after the load. That is retention, not growth.
- **Network and disk** are negligible: about 0.4 KB/s of upload and
  0.2 KB/s of spool writes at 100 lines/s.

## Reproduce

```sh
export B=<empty dir> FORGE=target/release/forge-server SIM=target/release/fleet-sim
export API=http://127.0.0.1:56130 AGENT_URL=https://127.0.0.1:56131
scripts/dev/capacity-bench/setup.sh <engine binary> <sha256> 2 2048 32
scripts/dev/capacity-bench/run_phase.sh p500 500 300 20 4 5 300
```
