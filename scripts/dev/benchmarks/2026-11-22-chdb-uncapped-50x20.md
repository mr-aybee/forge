# chDB uncapped: 50 nodes × 20 services (10 logging at 10 lines/s)

[← All benchmarks](README.md)

| | |
| --- | --- |
| Date | 2026-11-22 |
| Commit | `3a8bdf3` (adds `fleet-sim --logging-containers`) |
| System | 16 vCPU Xeon E5-2620 v2 (no AVX2), 62 GB RAM, local NVMe, Linux 6.8 |
| Flow | Agent spools → worker → embedded chDB; SQLite control plane |
| Store | chDB, **uncapped** (memory and CPU pinning off; `max_threads` and merge pool still 2) |
| Load | 50 simulated nodes × 20 containers, 10 logging at 10 lines/s = 5,000 lines/s; 4 min, with idle and after windows |
| Verdict | **Pass with room to spare**: about 2.6 cores and 0.5 GB for the whole flow, 5 s drain, search p95 403 ms. chDB CPU rose over the run (needs a longer soak) |

The owner's scenario, reduced to 50 nodes: each node runs 20 containers,
10 of which write 10 lines/s; the other 10 are silent but send their
per-minute samples. The goal is CPU and memory over time with no caps.

| Item | Value |
| --- | --- |
| Load | `run_phase.sh p50 50 240 100 4 20 300 10` with `IDLE_SECS=30 POST_SECS=120`: 30 s idle, 4 min load, drain, 2 min after |
| Engine | **Uncapped**: `cpu_quota_vcpu = 0`, `embedded_memory_mb = 1048576` (the maximum) |
| Verified on the engine | `Cpus_allowed_list: 0-15` (not pinned); `max_server_memory_usage` = 53.2 GiB (the engine clamps the budget to 0.9 × RAM); per-query `max_memory_usage` 1 TiB; `concurrent_threads_soft_limit_num = 0` |
| Still limited | `max_threads = 2`, `background_pool_size = 2`, `background_schedule_pool_size = 4`: Forge's defaults without a CPU cap (`Threads::for_cpus(None)`); there is no setting for "no limit" |
| `worker_max_uploads` | 8 (default) |
| Readers | 4 console users, as in [the capped run](2026-11-21-chdb-capped-node-scaling.md) |

### Result

| Offered/s | Acked/s | Stored/s | Sent | Acked | Stored | Distinct | Dropped | 429s | Other deferrals | Backlog max | Drain | Result |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 5,243 | 5,243 | 5,015 | 1,132,023 | 1,132,023 | 1,132,023 | 1,132,023 | 0 | 925 | 214 | 150 files (1.2 MB) | 5 s | **pass** |

The rows come from exactly 500 containers (50 × 10). Search p50 / p95
158 / 403 ms, histogram 331 / 500 ms, node history 33 / 90 ms; no failed
reads; no ERROR or WARN in the server log. Heartbeat p95 at most 79 ms.
"Other deferrals" are uploads fleet-sim retried after a non-429 answer or
timeout; every one was later acknowledged. Loopback traffic during the
load (uploads plus all local API and engine traffic) averaged 0.98 MB/s
(idle: 0.001 MB/s).

### CPU and memory over time

Every 15 s (sample period 5 s). CPU is % of one core. chDB memory is its
own memory (`RssAnon`); its mapped binary adds a constant ~520 MB of
`RssFile`. The load ran from 35 s to 275 s.

| t (s) | Phase | Server CPU | Server RSS MB | Worker CPU | Worker RSS MB | chDB CPU | chDB own MB | Rows stored | Spool files | lo MB/s |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 15 | idle | 0.2 | 35 | 0.2 | 8 | 16 | 98 | 0 | 0 | 0.00 |
| 30 | idle | 3.0 | 55 | 0.0 | 8 | 13 | 98 | 0 | 0 | 0.00 |
| 45 | load | 16.6 | 82 | 9.2 | 15 | 107 | 171 | 9,056 | 19 | 0.38 |
| 60 | load | 22.2 | 83 | 118.4 | 62 | 114 | 325 | 65,650 | 28 | 1.11 |
| 90 | load | 14.8 | 83 | 26.0 | 70 | 200 | 240 | 251,951 | 1 | 1.41 |
| 120 | load | 15.2 | 83 | 1.6 | 71 | 125 | 188 | 379,933 | 0 | 1.32 |
| 135 | load | 22.6 | 83 | 90.8 | 74 | 247 | 336 | 462,714 | 21 | 1.29 |
| 165 | load | 17.4 | 83 | 25.8 | 75 | 250 | 239 | 620,926 | 9 | 1.41 |
| 195 | load | 25.8 | 83 | 6.6 | 73 | 286 | 201 | 756,099 | 0 | 1.49 |
| 210 | load | 21.4 | 83 | 118.0 | 75 | 288 | 358 | 846,368 | 14 | 1.31 |
| 240 | load | 14.8 | 83 | 10.6 | 74 | 263 | 218 | 1,001,717 | 2 | 1.36 |
| 270 | load | 22.6 | 83 | 11.6 | 79 | 379 | 245 | 1,132,023 | 0 | 1.50 |
| 285 | after | 1.0 | 83 | 0.4 | 79 | 14 | 168 | 1,132,023 | 0 | 0.05 |
| 330 | after | 0.4 | 83 | 0.0 | 79 | 14 | 166 | 1,132,023 | 0 | 0.00 |
| 420 | after | 0.2 | 83 | 0.2 | 79 | 13 | 158 | 1,132,023 | 0 | 0.00 |
| 540 | after | 0.4 | 83 | 0.2 | 79 | 13 | 159 | 1,132,023 | 0 | 0.00 |
| 705 | after | 0.4 | 83 | 0.0 | 79 | 14 | 158 | 1,132,023 | 0 | 0.00 |

| Process | Idle | Steady load (avg) | Peak | 7 min after the load |
| --- | --- | --- | --- | --- |
| forge-server | 0–3 % CPU, 35–55 MB | 19 % CPU, 83 MB | 37 % CPU, 83 MB | 0.4 % CPU, 83 MB |
| Worker | 0 % CPU, 8 MB | 27 % CPU, 74 MB | 129 % CPU, 79 MB | 0.1 % CPU, 79 MB |
| chDB | 13–16 % CPU, 98 MB own | 235 % CPU, 236 MB own | 440 % CPU, 384 MB own | 13–15 % CPU, 158 MB own |

**Memory after the load.** chDB gives back most of what it used: from a
384 MB peak to 158 MB within 2 minutes, 60 MB above its idle level
(caches and the new parts' metadata). forge-server (83 MB) and the worker
(79 MB) keep their peak: allocator retention, flat, not growing.

**CPU.** chDB's CPU rose through the load (about 110 % at the start,
250–380 % at the end) at a constant 5,000 rows/s, because the table's
part count grows and merges get more work; with the engine unpinned it
used up to 4.4 cores, although `max_threads` and the merge pool are 2
(inserts run on their own threads). The worker's CPU comes in bursts
every ~75 s (up to 129 %), when the nodes' synchronised heartbeats seal
and upload at once; the 429s are those bursts.

**Verdict.** 50 nodes with 20 services each (10 logging at 10 lines/s)
keep up with room to spare: every line stored once, a 5 s drain, a
backlog of at most 150 small files, sub-second searches, and about 2.6
cores and 0.5 GB of memory for the whole flow. The rising chDB CPU over
4 minutes is worth a longer soak before sizing a 2-core cap for this
load: the earlier capped run reached the 200 % cap at a similar row rate.

## Reproduce

```sh
export B=<empty dir> FORGE=target/release/forge-server SIM=target/release/fleet-sim
export API=http://127.0.0.1:56130 AGENT_URL=https://127.0.0.1:56131
scripts/dev/capacity-bench/setup.sh <engine binary> <sha256> 2 2048 32
scripts/dev/capacity-bench/run_phase.sh p500 500 300 20 4 5 300
```
