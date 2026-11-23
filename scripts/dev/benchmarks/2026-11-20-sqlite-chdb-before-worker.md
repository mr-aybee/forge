# SQLite + chDB before the analytics worker (heartbeat telemetry)

[← All benchmarks](README.md)

| | |
| --- | --- |
| Date | 2026-11-20 |
| Commit | `develop` before D-141 (telemetry rode on heartbeats into `forge-server`) |
| System | 16 vCPU Xeon E5-2620 v2 (no AVX2), 62 GB RAM, local NVMe, Linux 6.8 |
| Flow | Agents → heartbeats with logs and metrics → `forge-server` (LogBatcher, buffers) → embedded chDB; SQLite control plane |
| Store | chDB, quiet config, CPU and memory limits as configured then |
| Load | fleet-sim, 20 log lines/s per node, phases up to 2,000 nodes, with console readers |
| Verdict | Held **~1,000 nodes** (about 20k lines/s); broke at 1,500–2,000. The backlog sat in `forge-server` memory (up to 4.4 GB). This led to the worker and agent spools (D-141, D-142) |

This run was stopped by the owner before it finished, so there is no
full per-phase table. The figures below are the ones recorded during the
run; the raw results were not kept in the repository.

## What was measured

| Measure | Result |
| --- | --- |
| Largest fleet that kept up | about 1,000 nodes at 20 lines/s (about 20k lines/s) |
| Break point | 1,500 to 2,000 nodes |
| `forge-server` RSS under backlog | up to 4.4 GB |
| `forge-server` RSS on a fresh process at 500 nodes | about 1 GB (RSS is a high-water mark; the 2,000-node phase ran before 1,500) |
| chDB own memory | about 150 to 415 MB, bounded, plus about 520 MB of mapped binary |

## Why it broke

- The backlog had nowhere to go but `forge-server` memory: the LogBatcher
  queue, request bodies plus decoded events, a second copy during the
  insert, and heartbeats held for up to 10 s.
- A container batch that timed out was stored twice.
- chDB itself stayed inside its memory; the server was the weak point.

## What changed because of it

- All analytics moved into `forge-analytics-worker` (D-141), so
  `forge-server` holds no analytics data.
- Agents spool to disk and upload straight to the worker (D-142), so a
  backlog waits on the nodes' disks, not in server memory.
- See [chDB capped, node scaling](2026-11-21-chdb-capped-node-scaling.md)
  for the same kind of run on the new flow.
