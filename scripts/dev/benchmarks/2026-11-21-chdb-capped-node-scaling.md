# chDB capped at 2 cores / 2 GB: node scaling with spools and the worker

[← All benchmarks](README.md)

| | |
| --- | --- |
| Date | 2026-11-21 |
| Commit | `bb0022d`, fixes `fdfca46` and `b474b73` applied during the run |
| System | 16 vCPU Xeon E5-2620 v2 (no AVX2), 62 GB RAM, local NVMe, Linux 6.8 |
| Flow | Agent spools → worker (mTLS upload) → embedded chDB; SQLite control plane |
| Store | chDB, **capped** 2 cores, 2 GB |
| Load | 50 → 2,000 simulated nodes × 20 lines/s, 5 containers each; 500 nodes at 3× |
| Verdict | Holds **~500 nodes** (10k rows/s) with 32 upload slots; falls behind at 1,000. No rows lost. Found two bugs (duplicates, search 503 above ~1,400 nodes), both fixed |

The analytics flow of D-141/D-142 under load: agents spool zstd NDJSON
files and upload them over mTLS to `forge-analytics-worker`, which
imports them into the embedded ClickHouse engine (chDB). `forge-server`
only supervises.

## Setup

| Item | Value |
| --- | --- |
| Host | 16 vCPU (no AVX2, `amd64compat` engine build 26.12.1), 62 GB RAM, local NVMe |
| Build | `cargo build --release -p forge-server -p forge-agent -p forge-analytics-worker -p fleet-sim` |
| Operational store | SQLite |
| Analytics | `engine = "embedded"`, `embedded_memory_mb = 2048`, `cpu_quota_vcpu = 2` |
| Caps verified | `max_server_memory_usage = 2147483648`; engine `Cpus_allowed_list: 0-1`; `max_threads = 2`; `background_pool_size = 2` |
| Not capped | `forge-server` (with `MALLOC_ARENA_MAX=2`, like the systemd unit), the worker |
| Upload port | `worker_agent_listen = 127.0.0.1:56132`, `worker_agent_url = https://127.0.0.1:56132`; `127.0.0.1` is in `agent_hostnames` |
| `worker_max_uploads` | 4 (default) for phases 1–3, then **32** |
| Load | `fleet-sim` soak, `--log-lines-per-sec 20 --containers 5 --server-pid --settle-secs`; each node also writes node minutes, top processes and 5 container minutes |
| Readers | 4 console users, each every 2 s: servers list, log search, log histogram (15 min), node history, container metrics |
| Scripts | `scripts/dev/capacity-bench/` (`setup.sh`, `run_phase.sh`, `sample.py`, `readers.py`, `summarize.py`, `rows.sh`, `events.sh`; for a real node `containers.sh`, `agent_sample.py`, `agent_summary.py`) |

Stored rows are counted in the engine (`app_logs`, `source = 'container'`).
Duplicates are counted as `count()` minus distinct
`(server_id, ts, message, container_id, stream)`. chDB memory is shown as
its own memory (`RssAnon`); the mapped binary adds a constant ~520 MB of
`RssFile`. CPU is % of one core; the engine's cap is 200 %.

## Results per phase

Steady state after a 60 s warm-up. "Acked" is lines in files the worker
acknowledged; "drain" is the time from load stop until no `.ready` file is
left in any node's spool.

| Phase | Nodes × lines/s | Offered/s | Acked/s | Sent | Acked | Stored | Dropped | 429s | Backlog max (files) | Drain | Result |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| p50 | 50 × 20 | 963 | 907 | 167,875 | 167,875 | 167,875 | 0 | 679 | 150 | 5 s | pass |
| p250 | 250 × 20 | 4,750 | 4,050 | 1,098,287 | 1,098,287 | 1,098,287 | 0 | 7,035 | 595 | 66 s | pass (at the edge) |
| p500 | 500 × 20, uploads 4 | 10,024 | 4,155 | 2,866,991 | 2,766,767 | 2,767,769 | 0 | 13,816 | 4,120 | > 300 s | **fail** |
| p500b | 500 × 20, uploads 32 | 10,124 | 9,793 | 2,927,379 | 2,927,379 | 3,268,526 | 0 | 13,597 | 1,488 | 49 s | **fail: duplicates** |
| p500c | 500 × 20, uploads 32 + fix | 10,105 | 9,972 | 2,921,406 | 2,921,406 | 2,921,406 | 0 | 20,145 | 957 | 44 s | **pass** |
| p1000 | 1,000 × 20 | 20,511 | 13,989 | 4,578,945 | 4,578,945 | 4,578,945 | 0 | 27,180 | 3,357 | 119 s | **fail: falls behind** |
| p1500 | 1,500 × 20 | 29,910 | 16,033 | 6,790,874 | 6,790,874 | 6,790,874 | 0 | 37,564 | 7,582 | 183 s | fail; all searches 503 |
| p2000 | 2,000 × 20 | 39,936 | 15,541 | 9,057,519 | 9,057,519 | 9,057,519 | 0 | 52,239 | 12,764 | 282 s | fail; all searches 503 |
| p2000b | 2,000 × 20 + search fix | 39,379 | 11,297 | 9,305,972 | 9,194,295 | 9,198,646 | 0 | 54,138 | 17,602 | > 300 s | fail; searches work but slow |
| p500x3 | 500 × 60, 2 min | 32,776 | 29,259 | 3,090,988 | 3,090,988 | 3,090,988 | 0 | 8,856 | 661 | 59 s | pass for ingest; histograms time out |

Stored above acked (p500, p2000b) is files stored while their ack was on
the wire when the soak stopped. They were acknowledged without a second
write on the next run (seen-id log); the full-history check after the
final drain found duplicates only from p500b (241,925 rows, before the fix).

### Processes

| Phase | Server RSS MB (max) | Server CPU % (avg) | Server fds | Worker RSS MB (max) | Worker CPU % (avg) | chDB own MB (max, cap 2048) | chDB CPU % (avg, cap 200) | Heartbeat p95 max ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| p50 | 83 | 18 | 85 | 23 | 6 | 215 | 89 | 185 |
| p250 | 101 | 48 | 288 | 42 | 16 | 331 | 183 | 204 |
| p500 | 115 | 58 | 538 | 61 | 16 | 448 | 199 | 162 |
| p500b | 112 | 32 | 538 | 103 | 85 | 597 | 200 | 659 |
| p500c | 84 | 53 | 539 | 104 | 52 | 462 | 197 | 644 |
| p1000 | 160 | 42 | 1,039 | 140 | 86 | 504 | 200 | 547 |
| p1500 | 206 | 330 | 1,539 | 168 | 154 | 494 | 186 | 1,527 |
| p2000 | 248 | 340 | 2,039 | 204 | 141 | 551 | 193 | 1,969 |
| p2000b | 230 | 108 | 2,038 | 173 | 96 | 528 | 199 | 5,488 |
| p500x3 | 260 | 52 | 538 | 287 | 224 | 637 | 200 | 629 |

forge-server was not restarted between p2000b and p500x3, so its RSS in
p500x3 is allocator retention from 2,000 nodes, not the 500-node load.

### Console reads

| Phase | Search p50 / p95 ms | Histogram p50 / p95 ms | Failed reads | Log ERROR / WARN |
| --- | --- | --- | ---: | --- |
| p50 | 152 / 300 | 153 / 277 | 0 | 0 / 0 |
| p250 | 458 / 1,329 | 670 / 2,271 | 0 | 0 / 1 |
| p500 | 800 / 2,267 | 1,924 / 4,905 | 0 | 0 / 50 |
| p500b | 1,813 / 6,060 | 7,663 / 15,890 | 116 (503) | 176 / 3 |
| p500c | 1,009 / 3,498 | 1,884 / 6,113 | 0 | 0 / 1 |
| p1000 | 4,044 / 17,909 | 10,782 / 20,258 | 2 | 2 / 3 |
| p1500 | all 503 | all 503 | 335 | 418 / 216 |
| p2000 | all 503 | all 503 | 240 | 296 / 3,512 |
| p2000b | 3,588 / 10,590 | 12,661 / 19,872 | 0 | 4 / 7,555 |
| p500x3 | 4,697 / 21,257 | 20,724 / 21,496 | 12 (timeouts) | 20 / 128 |

WARN at 50 nodes and up is mostly "the agent restarted after taking these
commands" (simulated nodes reconnecting between phases). From 1,500 nodes
on, nearly all WARN are SQLite pool slow acquires and slow
`UPDATE servers SET last_seen_at` statements.

### Overload and exactly once

500 nodes at 60 lines/s (3×) for 2 minutes, then no load: 3,090,988 lines
sent, acked and stored, 3,090,988 distinct; spools drained 59 s after the
load stopped; no drops. Over the whole benchmark (43,121,929 rows) the only
duplicates are the 241,925 rows of p500b.

## Where it breaks, and why

**With chDB capped at 2 cores and 2 GB, the flow holds about 500 nodes
at 20 lines/s per node (10,000 rows/s). At 1,000 nodes it falls behind.**

1. **Upload slots, at the default.** With `worker_max_uploads = 4` the
   worker imported ~4,200 rows/s and chDB already sat at 200 % CPU, so it
   looked engine-bound. Each import waits for `async_insert` to flush
   (`wait_for_async_insert=1`), so 4 slots serialise the fleet. With 32
   slots the same engine took ~10,000 rows/s: more inserts per flush make
   larger parts and fewer merges.
2. **The engine's query limit (fixed, `fdfca46`).** The engine admitted 32
   queries in total. 32 imports plus searches and flushes went past it;
   the engine refused with `TOO_MANY_SIMULTANEOUS_QUERIES`, a file refused
   after its first table insert was stored again on resend (241,925
   duplicate rows in p500b), and searches got 503. Now inserts (72) and
   reads (24) are limited apart, and every import fits.
3. **chDB CPU.** After that, 2 cores are the limit. Merges take about two
   thirds of the engine's CPU time (rows are rewritten 7–24× while small
   async-insert parts merge). Ingest peaks at 14–16k rows/s at
   1,000–1,500 nodes, then falls to ~11k rows/s at 2,000 once searches
   work again and take their share. The same 500 nodes at 3× the rate
   reached 29k rows/s: throughput follows rows per insert, not rows.
   At 20 lines/s each node seals a small file per heartbeat, so the
   per-insert cost dominates.
4. **Log search above ~1,400 nodes (fixed, `b474b73`).** A search sends the
   list of nodes the caller may view as a URL parameter. Past ~1,400 nodes
   the URL passed 64 KiB and every search answered 503. Parameters now go
   in a form body.
5. **Reads under ingest load.** Searches share the 2 cores. Search p95 is
   3.5 s at 500 nodes and 10–18 s at 1,000–2,000; histograms hit the 20 s
   timeout under 3× load.
6. **Control plane at 1,500–2,000 nodes.** Not analytics: SQLite pool
   waits (up to 7,555 slow acquires per phase), slow `last_seen_at`
   updates, heartbeat p95 up to 5.5 s and 3–4 cores of server CPU in
   p1500/p2000. RSS grows ~60 KB per node (248 MB at 2,000).

Nothing was lost in any phase: the spools absorbed every backlog and
drained. Past the break point the backlog grows without bound and would
hit the 512 MiB per-node spool cap after hours, when the oldest files are
dropped.

## Recommendations

- **`worker_max_uploads` is now 8 to 32, default 8.** At 4 the worker,
  not the engine, was the limit (32 gave 2.4× the throughput at 500 nodes
  for the same CPU). The cap stays at 32 because each slot is an upload in
  flight, so more slots use more network. Raise it toward 32 for large
  fleets.
- **Make inserts larger.** Group rows from several uploaded files into
  one insert per table in the worker, or seal agent files less often
  (larger `seal_secs` at low line rates); either cuts per-insert and merge
  cost. The 3× run shows the engine has the headroom.
- **Make each import idempotent per table** (`insert_deduplication_token`
  from the file id and table, with `async_insert_deduplicate`), so a
  failure between two table inserts no longer stores the written part
  twice on resend.
- **Size the engine by fleet**: 2 cores for up to ~500 nodes at 20
  lines/s; 4+ cores (or an external ClickHouse) beyond that.
- **Protect reads**: a lower `search.timeout_secs` / `max_threads` for
  histograms, or a separate read slot pool, so ingest cannot starve the
  console.
- **Control plane past 1,000 nodes**: batch the `last_seen_at` updates (or
  move them off SQLite's single writer); look at the server CPU jump
  between 1,000 and 1,500 nodes.
- **fleet-sim**: nodes' heartbeats are synchronised, so every node seals
  and uploads in the same second; the 429 counts are partly that burst.
  Real agents spread out.

## Reproduce

```sh
export B=<empty dir> FORGE=target/release/forge-server SIM=target/release/fleet-sim
export API=http://127.0.0.1:56130 AGENT_URL=https://127.0.0.1:56131
scripts/dev/capacity-bench/setup.sh <engine binary> <sha256> 2 2048 32
scripts/dev/capacity-bench/run_phase.sh p500 500 300 20 4 5 300
```
