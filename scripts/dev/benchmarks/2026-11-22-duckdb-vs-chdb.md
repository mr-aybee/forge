# DuckDB against chDB: idle, light, 50 and 100 nodes, and DuckDB's pressure point

[← All benchmarks](README.md)

| | |
| --- | --- |
| Date | 2026-11-22 |
| Commit | `4877359` (DuckDB store, D-143) for the first phases; the tuning of D-144 for `d_p50_tuned` and `d_p100_tuned` |
| System | 16 vCPU Xeon E5-2620 v2 (no AVX2), 62 GB RAM, local NVMe, Linux 6.8 |
| Flow | Agent spools → worker → DuckDB inside the worker, or → embedded chDB; SQLite control plane |
| Store | DuckDB (`engine = "duckdb"`, `memory_limit` 256 MB, 2 threads); chDB uncapped for the comparison |
| Load | Idle 2 min; 5 nodes × 20 containers (5 logging at 2/s); 50 and 100 nodes × 20 containers (10 logging at 10/s); then 100 nodes at 20/s and 40/s per container |
| Verdict | **DuckDB holds 100 nodes × 10 lines/s** (about 9,000 lines/s) with every line stored once and a 4 s drain after tuning, at near-zero idle cost (0.0–0.2 % CPU, 44 MB). It breaks at about 18,000 lines/s with console readers: wide text searches scan every row and compete with ingest. chDB searches stay under 1 s at the same load |

Each phase ran `scripts/dev/capacity-bench/run_phase.sh` with 4 console
users polling log search, log histogram (15 minutes), node history and
container metrics every 2 s, except where it says "no readers". DuckDB
locks its files, so the stored rows were counted after each phase with
`forge-analytics-duckdb`'s `duck_count` example while the server was
stopped (then restarted for the next phase). Distinct is by
`(server_id, ts, message, container_id, stream)`. CPU is % of one core;
DuckDB's CPU is the worker's, since DuckDB runs inside it. chDB's memory
is its own (`RssAnon`) plus a constant ~520 MB mapped binary.

## Results per phase

| Phase | Load | Offered/s | Acked/s | Sent | Stored | Distinct | Drain | 429s | Backlog max (files) | Result |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `d_light` | 5 × 20, 5 logging at 2/s, after 2 min idle | 52 | 52 | 14,471 | 14,471 | 14,471 | 5 s | 0 | 10 | pass |
| `d_p50` | 50 × 20, 10 logging at 10/s | 5,592 | 5,594 | 1,197,157 | 1,197,157 | 1,197,157 | 25 s | 1,084 | 122 | pass |
| `d_p100` | 100 × 20, 10 at 10/s | 9,233 | 8,872 | 2,193,697 | 2,193,697 | 2,193,697 | 34 s | 3,280 | 187 | edge: histogram p95 7.8 s |
| `d_p100x20` | 100 × 20, 10 at 20/s | 18,496 | 15,681 | 4,405,343 | 4,405,343 | 4,405,343 | 95 s | 3,032 | 214 | **break**: acked < offered, spools grow, search p95 5.3 s |
| `d_p100x20nr` | same, **no readers** | 18,527 | 18,186 | 4,411,600 | 4,411,600 | 4,411,600 | 24 s | 3,417 | 130 | pass: ingest alone keeps up |
| `d_p100x40` | 100 × 20, 10 at 40/s | 37,066 | 16,171 | 8,825,420 | 8,825,420 | 8,825,420 | 230 s | 2,404 | 589 | break: acked plateaus at ~16k/s; 6 histograms 503 |
| `c_p100` | chDB uncapped, 100 × 20, 10 at 10/s | 10,563 | 10,170 | 2,330,451 | 2,330,451 | 2,330,451 | 28 s | 2,797 | 157 | pass |
| `d_p50_tuned` | as `d_p50`, after D-144 | 5,589 | 5,589 | 1,197,634 | 1,197,634 | 1,197,634 | 20 s | 507 | 138 | pass |
| `d_p100_tuned` | as `d_p100`, after D-144 | 9,242 | 8,884 | 2,193,804 | 2,193,804 | 2,193,804 | 4 s | 1,866 | 201 | pass |

Nothing was lost or stored twice in any phase. Upload failures other than
429 (22–308 per phase) were `403 unknown node` while new nodes were not yet
in the active-node list the server pushes every 15 s; each file was
retried and acknowledged. The backlog peaks come from fleet-sim's nodes
sealing their files at the same moment; real nodes drift apart.

### Processes, disk and searches

| Phase | Worker CPU avg / max | Worker RSS max | chDB CPU avg / own MB | Write MB/s | Search p50 / p95 | Histogram p50 / p95 | Node history p95 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `d_light` | 25 / 33 % | 75 MB | | 0.02 | 40 / 53 ms | 31 / 40 ms | 35 ms |
| `d_p50` | 241 / 406 % | 215 MB | | 1.8 | 156 / 592 ms | 415 / 778 ms | 103 ms |
| `d_p100` | 483 / 542 % | 341 MB | | 3.9 | 238 ms / 2.4 s | 2.6 / 7.8 s | 111 ms |
| `d_p100x20` | 531 / 576 % | 354 MB | | 5.9 | 386 ms / 5.3 s | 4.6 / 11.7 s | 119 ms |
| `d_p100x20nr` | 125 / 197 % | 341 MB | | 10.4 | | | |
| `d_p100x40` | 516 / 577 % | 388 MB | | 5.8 | 263 ms / 7.1 s | 9.0 / 20.0 s | 137 ms |
| `c_p100` | 43 / 133 % | 90 MB | 297 % / 392 MB | 2.8 (chDB) | 196 / 438 ms | 391 / 762 ms | 78 ms |
| `d_p50_tuned` | 269 / 500 % | 251 MB | | 1.5 | 179 / 922 ms | 430 / 967 ms | 84 ms |
| `d_p100_tuned` | 346 / 547 % | 368 MB | | 3.5 | 1.7 / 3.7 s | 3.1 / 5.6 s | 97 ms |

DuckDB's `memory_limit` was 256 MB throughout. Worker RSS above it is the
worker's own import buffers, connections and allocator retention; it fell
to 93–170 MB after each load.

**Disk.** The logs day file held 12.2 million lines in 1.12 GB (at most
92 B per line, with free blocks DuckDB reuses), including the
`spool_files` ids. During the load DuckDB's write throughput was 1.8 MB/s
at 5,600 lines/s and 10 MB/s at 18,500 lines/s without readers. chDB grew
its data by about 400 MB for 2.33 million lines while its parts were not
yet merged.

### Idle and light load

| | Worker CPU | Worker RSS |
| --- | ---: | ---: |
| Idle, no nodes (2 min) | 0.0–0.2 % | 44 MB |
| 5 nodes light, **no readers** | 0.5 % (0–5 %) | 50–70 MB |
| 5 nodes light, 4 readers every 2 s | 25 % | 75 MB |
| chDB idle (earlier run) | worker 0.2 % + chDB 13–16 % | 8 MB + 98 MB own + ~520 MB mapped |

The light-load CPU is the console readers, not ingest. Measured on the
light store, each request costs the worker:

| Endpoint | Worker CPU per request | Wall |
| --- | ---: | ---: |
| Log search | 39.9 ms | 55.8 ms |
| Log histogram | 45.2 ms | 60.5 ms |
| Node history | 22.8 ms | 27.0 ms |
| Container metrics | 37.3 ms | 41.2 ms |

Four users polling four endpoints every 2 s are about 8 DuckDB queries a
second, so 20–25 % of a core. With `duck_bench`, the same reads cost 9–26 ms
at 1 thread and 14–38 ms at 8: DuckDB's parallel scheduling dominates on
small data. A checkpoint and late materialisation (top ids first) did not
lower it. No timer, flush or checkpoint costs anything at idle.

## DuckDB's pressure point

- **Search contention breaks first.** At 100 nodes × 20 lines/s with readers,
  acked fell to 85 % of offered and search p95 passed 5 s. The same load
  without readers was acked at 98 % with a 24 s drain. The 15-minute log
  histogram with a text filter scans every row of the window
  (`contains(lower(message), ..)` over about 15 million rows), because
  DuckDB has no token index for text. On 16 idle threads it takes 0.5 s;
  on 2 threads next to ingest, several seconds.
- **The single writer comes next.** Its thread used 50 % of a core at
  9,000 lines/s, 57 % (peaks 88 %) at 18,500 lines/s without readers, and
  peaked at 99 % at 37,000 lines/s offered, where acked plateaued at about
  16,000 lines/s. Ingest alone tops out around 20,000–25,000 lines/s.
- **Not the limit:** `memory_limit` (no spills, no errors), disk I/O (10 MB/s
  at most), or the server.

What moves it: fewer concurrent searches or a narrower default window
(searches), more DuckDB threads for searches (CPU), or chDB/ClickHouse for
text search at this volume.

## Tuning (D-144) before and after

| | `d_p50` → `d_p50_tuned` | `d_p100` → `d_p100_tuned` |
| --- | --- | --- |
| Drain | 25 s → 20 s | 34 s → **4 s** |
| Backlog at load stop | 100 → 75 files | 6 → **0** files |
| 429s | 1,084 → 507 | 3,280 → 1,866 |
| Worker CPU avg | 241 % → 269 % | 483 % → **346 %** |
| Writer thread avg | | 50 % → 28 % |
| Search p95 | 592 → 922 ms | 2.4 → 3.7 s |
| Histogram p95 | 778 → 967 ms | 7.8 → **5.6 s** |
| Node history p95 | 103 → 84 ms | 111 → 97 ms |

The changes:

- Group commit with no timer: everything queued while a batch commits forms
  the next batch.
- 32 upload slots by default with DuckDB.
- DuckDB queries capped at its thread count per kind, with log searches and
  history reads in separate pools. A first try with one shared pool made
  node history wait behind slow histograms (p95 2.1 s); separate pools
  fixed it.

Searches trade a little latency at 50 nodes for keeping ingest ahead at
100 nodes.

## Verdict

DuckDB is the right default for small and medium installs: nothing to run
or download, near-zero idle cost, and every line stored once up to 100
nodes × 10 lines/s with a 4 s drain. Use chDB or ClickHouse when log volume
passes about 10,000 lines/s or when wide text searches must stay under a
second.

## Reproduce

```sh
export B=<empty dir> FORGE=target/release/forge-server SIM=target/release/fleet-sim
export API=http://127.0.0.1:56130 AGENT_URL=https://127.0.0.1:56131
cargo build --release -p forge-analytics-duckdb --example duck_count --example duck_bench
ENGINE=duckdb scripts/dev/capacity-bench/setup.sh - - 2 256 32
IDLE_SECS=30 POST_SECS=120 scripts/dev/capacity-bench/run_phase.sh d_p50 50 240 100 4 20 300 10
# stop forge-server by its pid, then:
DUCK_COUNT=target/release/examples/duck_count scripts/dev/capacity-bench/rows_duck.sh
```
