---
title: Benchmarks
description: How resource use is measured with fleet-sim, and current results.
sidebar:
  order: 3
---

`src/forge/tools/fleet-sim` starts a real control plane, enrolls N agents with real
keys, certificates and mTLS connections, and records idle RSS/CPU, API latency
and reconnect-storm recovery.

```sh
cd src/forge
cargo build --release -p forge-server -p forge-agent -p fleet-sim
target/release/fleet-sim --agents 50 --agents 1000 --idle-secs 60 --out bench.json
```

Benchmarks are run by hand on a developer machine; CI does not run them.
Do not run them on the shared CI runner.

## Latest results (Windows 11 workstation, SQLite)

| Agents | CP RSS max (MB) | CP CPU (% core) | API p99 (ms) | Reconnect storm (s) |
|---:|---:|---:|---:|---:|
| 50 | 24.0 | 0.27 | 38.4 | 0.0 |
| 1000 | 72.8 | 4.36 | 1720.5 | 8.9 |

Idle `forge-agent`: 17.2 MB RSS, 0.22 % CPU. Targets: agent ≤ 25 MB and < 1 %
CPU; control plane ≤ 150 MB with 50 agents — all met. Every heartbeat now
carries a resource sample for node telemetry, which accounts for most of the
extra cost at 1,000 agents.

The full table, findings (API latency grows with fleet size; fix planned) and
the list of runs still to do are in `src/forge/BENCHMARKS.md`.

## Analytics: DuckDB against chDB

Measured on a 16 vCPU Xeon E5-2620 v2 (no AVX2) with 62 GB RAM and local
NVMe, Linux 6.8, SQLite control plane, with `fleet-sim` nodes spooling to the
analytics worker and four console users polling log search, the log
histogram, node history and container metrics every 2 s. DuckDB ran with
`memory_limit` 256 MB and 2 threads, after the tuning of D-144; chDB had no
limit. CPU is % of one core.

| Load | Engine | Lines/s stored | Worker CPU avg | Worker RSS max | Search p95 | Histogram p95 | Result |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| Idle | DuckDB | 0 | 0.0–0.2 % | 44 MB | | | pass |
| Idle | chDB | 0 | 13–16 % | about 600 MB with its mapped binary | | | pass |
| 5 nodes, 5 containers logging at 2/s | DuckDB | 52 | 25 % (the readers; ingest alone 0.5 %) | 75 MB | 53 ms | 40 ms | pass |
| 50 nodes × 10 containers at 10/s | DuckDB | 5,589 | 269 % | 251 MB | 922 ms | 967 ms | pass |
| 100 nodes × 10 containers at 10/s | DuckDB | 8,884 | 346 % | 368 MB | 3.7 s | 5.6 s | pass |
| 100 nodes × 10 containers at 10/s | chDB | 10,170 | 43 % + chDB 297 % | 90 MB + chDB 392 MB | 438 ms | 762 ms | pass |
| 100 nodes × 10 containers at 20/s | DuckDB | 15,681 of 18,496 offered | 531 % | 354 MB | 5.3 s | 11.7 s | break: spools grow |
| the same, no console readers | DuckDB | 18,186 | 125 % | 341 MB | | | pass |

Every line was stored exactly once in every phase. DuckDB is the default:
it costs nothing at idle and holds about 100 nodes with moderate logging.
Wide log searches scan every row and compete with ingest, so at about
18,000 lines/s with people searching, choose `embedded` (chDB) or a
ClickHouse server. The full report, with the method and every phase, is
`scripts/dev/benchmarks/2026-11-22-duckdb-vs-chdb.md` in the source
repository.
