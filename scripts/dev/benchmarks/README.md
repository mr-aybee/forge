# Benchmarks

One file per benchmark run, newest first. Each file starts with the same
block (date, commit, system, flow, store, load, verdict), so runs can be
compared by day, system, store and load.

`report.html` shows every run in one page with charts:
throughput against load, CPU and memory over time, chDB against DuckDB,
the agent on a real node, and each store's pressure point. Open it in a
browser; it works offline. `report.html` and `report/data.json` are
generated and git-ignored, not committed. Build them from your results:

```sh
python3 -I scripts/dev/benchmarks/report/extract.py <results-root> > scripts/dev/benchmarks/report/data.json
python3 -I scripts/dev/benchmarks/report/build_report.py
```

| Date | Report | Store | Load | Verdict |
| --- | --- | --- | --- | --- |
| 2026-11-22 | [DuckDB against chDB](2026-11-22-duckdb-vs-chdb.md) | DuckDB (day files), chDB uncapped | Idle, 5 nodes light, 50 and 100 nodes × 10 logging at 10/s, then 20/s and 40/s; tuned reruns | DuckDB idle 0.0 %, 44 MB; holds 100 nodes × 10/s (4 s drain after tuning); breaks at ~18k lines/s on search contention |
| 2026-11-22 | [50 nodes with a real agent](2026-11-22-chdb-real-agent.md) | chDB, uncapped | 49 simulated + 1 real agent with 20 Docker containers, 5,000 lines/s, 60 s outage | Agent 0.6 % of a core, 20 MB; exactly once through the outage |
| 2026-11-22 | [50 nodes × 20 services, uncapped](2026-11-22-chdb-uncapped-50x20.md) | chDB, uncapped | 50 nodes × 20 containers, 10 logging at 10/s (5,000 lines/s) | Pass; about 2.6 cores and 0.5 GB for the flow |
| 2026-11-21 | [chDB capped, node scaling](2026-11-21-chdb-capped-node-scaling.md) | chDB, 2 cores / 2 GB | 50 → 2,000 nodes × 20 lines/s, 3× overload | Holds ~500 nodes; no rows lost; 2 bugs fixed |
| 2026-11-21 | [Seq in Docker](2026-11-21-seq-docker.md) | Seq (comparison) | 2,000 events/s for 180 s | About 17 % of a core, 140 MiB |
| 2026-11-20 | [SQLite + chDB before the worker](2026-11-20-sqlite-chdb-before-worker.md) | chDB via `forge-server` | Up to 2,000 nodes × 20 lines/s | Held ~1,000 nodes; server memory up to 4.4 GB |

Control-plane benchmarks (enrolment, API latency, reconnect after a
restart, the 1,000-node scale test) are in [`BENCHMARKS.md`](../../../BENCHMARKS.md)
at the repository root.

## Systems

| Name | Hardware | Used by |
| --- | --- | --- |
| Dev host | 16 vCPU Xeon E5-2620 v2 (no AVX2), 62–64 GB RAM, local NVMe, Linux 6.8 | Every run above |

## Adding a run

1. Copy the header block from the newest file. Name the file
   `YYYY-MM-DD-<store>-<what>.md`.
2. Fill in date, commit, system, flow, store, load and verdict. Say
   which limits were set and how you checked them on the running engine.
3. Give results as tables: per phase, per process, and over time. Show
   CPU as % of one core and say what memory figure you mean (RSS, own,
   mapped).
4. Say plainly where it breaks and why, and what would move the limit.
5. Add a row to the table above, newest first, and a row to **Systems**
   for new hardware.
6. Add the run to `report/extract.py` and rebuild `report.html`.

Scripts: `scripts/dev/capacity-bench/` (Forge flow) and
`scripts/dev/seq-bench/` (Seq).
