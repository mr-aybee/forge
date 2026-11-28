# Benchmarks

Measured with `src/tools/fleet-sim` against release builds (LTO, codegen-units=1,
panic=abort) on SQLite. Each run starts a real `forge-server` process, enrolls N
agents (each with its own key, certificate and HTTP/2 mTLS long-poll
connection), samples the control plane for 60 s idle, measures 200 sequential
`GET /api/v1/servers`, then hard-kills and restarts the server and times until
every agent has reconnected.

**Environment:** Windows 11 (x86_64), single developer workstation, all agents
simulated in one process on the same host, 2026-11-09, commit after `82b741d`.
Linux VM numbers are still to be recorded (see "Not yet measured").

## Scale test: 1,000 nodes, 15,000 apps (`tests/it/scale.rs`)

`cargo test --profile perf -p forge-server --test it scale:: -- --ignored --nocapture`

The test seeds the database with:
- 1,000 nodes in 10 clusters;
- 300 namespaces;
- 15,000 apps with 30,000 placements;
- 3,000 installs.

It checks the query plan of every per-request lookup (failing on a full table
scan) and times each endpoint over HTTP: p50 and p99 of 10 runs. Measured on
the Windows workstation, idle.

| Endpoint | Before | After | Items |
| --- | ---: | ---: | ---: |
| `/servers` | 207 ms | **63 ms** | 1,000 |
| `/applications` (everything) | 1,456 ms | **651 ms** | 15,000 (6.9 MB) |
| `/applications?namespace_id=` | 422 ms | **5.5 ms** | 50 |
| `/applications?cluster_id=` (sidebar, Services) | — | **92 ms** | 1,500 |
| `/installations` | 577 ms | **123 ms** | 3,000 |
| `/clusters/{id}/fabric` | 96 ms | **48 ms** | 1 |
| `/clusters`, namespaces, jobs, audit | 2–30 ms | 1–13 ms | — |

**What was slow, and the fixes:**

| Cause | Fix |
| --- | --- |
| **Authorization per row.** Each server or app re-read its ancestry (~4 queries) and built its own Cedar entity set. | One batch per request: one query for the rows, memoized parent chains, and one merged entity set for all Cedar decisions. Tests prove the decisions are identical. |
| **One placement query per app** | One query for all of them (or one join per namespace or cluster). |
| **Cluster filter in memory** | A SQL join, plus the missing `namespaces(cluster_id)` index. |
| **The UI loaded every app for the sidebar** | The sidebar and Services pages ask for one cluster's apps. |

**Remaining cost:** decoding rows through sqlx's generic `Any` driver takes
about 7–15 µs per row (15,000 apps: 204 ms). The unfiltered `/applications`
list is the only endpoint that reads that many rows. The console no longer
calls it on normal pages.

## Results (2026-11-14, after the #15 file-transfer changes)

Windows 11 workstation, idle machine, commit `cb5cb5c`.

| Agents | Enroll (s) | All online (s) | CP RSS avg / max (MB) | CP CPU avg (% of one core) | API p50 / p99 (ms) | Reconnect storm (s) | DB (MB) |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 50 | 1.5 | 0.3 | 26.6 / 28.8 | 0.28 | 5.9 / 31.3 | 0.1 | 0.71 |
| 1000 | 19.9 | 0.5 | 85.9 / 87.1 | 4.04 | 85.4 / 137.9 | 12.0 | 2.90 |

Real `forge-agent` process: **not sampled in this run.** The real agent failed to
enroll against the benchmark server, so the agent figures in "Against the
targets" are from the previous run.

Release binary sizes (Windows x86_64): `forge-agent` 6.7 MB, `forge-agent-helper` 4.7 MB,
`forge-server` 27.8 MB, `forge-plugin-runner` 10.2 MB.

**Compared with the previous run** (below): at 1,000 agents, API p50 fell from
644 to 85 ms and p99 from 1,721 to 138 ms, after the server list started
authorizing in one batch (see the scale test above). Control-plane memory grew
from 72.8 to 87.1 MB, and the reconnect storm from 8.9 to 12.0 s. The
`forge-agent-helper` binary grew from 0.3 to 4.7 MB and `forge-server` from
14.3 to 27.8 MB.

## Results (2026-11-09, after node telemetry, fabric and live output)

Windows 11 workstation, idle machine, commit `764c4bd`.

| Agents | Enroll (s) | All online (s) | CP RSS avg / max (MB) | CP CPU avg (% of one core) | API p50 / p99 (ms) | Reconnect storm (s) | DB (MB) |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 50 | 0.5 | 0.3 | 24.0 / 24.0 | 0.27 | 15.5 / 38.4 | 0.0 | 0.44 |
| 1000 | 19.1 | 1.1 | 72.7 / 72.8 | 4.36 | 644.3 / 1720.5 | 8.9 | 2.20 |

Real `forge-agent` process, idle, connected: **RSS 17.1 MB avg (17.2 MB max), CPU 0.22 % of one core.**

**Compared with the run before it** (below): idle memory is about the same, and
50 agents are unchanged. At 1,000 agents, API p50 went from 196 to 644 ms, the
reconnect storm from 2.4 to 8.9 s, and control-plane CPU from 2.4 % to 4.4 %.
The agent's memory went from 10.6 to 17.2 MB, still within the 25 MB target.
The main new cost is **node telemetry** (`54e6cd1`): every heartbeat now carries a
resource sample (CPU, memory, disks via `sysinfo`), which the control plane
records. The `GET /api/v1/servers` handler itself is unchanged; it still
authorizes each server separately (see Findings), and now competes with the
telemetry load.

**Next steps:**
1. Authorize the server list in one pass: resolve ancestry with one query and evaluate Cedar on the batch.
2. Send telemetry every few heartbeats when nobody is watching the node (live charts already shorten the hold).
3. Reuse the `sysinfo` handles in the agent sampler instead of re-reading every disk.

## Previous results

| Agents | Enroll (s) | All online (s) | CP RSS avg / max (MB) | CP CPU avg (% of one core) | API p50 / p99 (ms) | Reconnect storm (s) | DB (MB) |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 50 | 0.6 | 0.4 | 21.3 / 21.4 | 0.38 | 8.1 / 25.6 | 0.1 | 0.30 |
| 1000 | 8.6 | 3.2 | 66.3 / 66.5 | 2.40 | 196.4 / 537.8 | 2.4 | 2.05 |

Real `forge-agent` process, idle, connected (Windows): **RSS 10.3 MB avg (10.6 MB max), CPU 0.05 % of one core.**

Release binary sizes (Windows x86_64): `forge-agent` 4.1 MB, `forge-agent-helper` 0.3 MB,
`forge-server` 14.3 MB, `forge-plugin-runner` 10.8 MB.

## Against the targets

| Target | Result | Status |
| --- | --- | --- |
| Agent idle RSS ≤ 25 MB | 17.2 MB max (previous run; not sampled in the latest) | ✅ |
| Agent idle CPU < 1 % of one core | 0.22 % (previous run; not sampled in the latest) | ✅ |
| Control plane ≤ 150 MB idle with 50 agents | 28.8 MB max | ✅ |
| Control plane with 1,000 agents | 87.1 MB max | ✅ (no target; well under 150 MB) |
| No Redis / broker / microservices | one process + SQLite | ✅ |

## Findings

- **API latency at 1,000 agents** is p50 85 ms, p99 138 ms now that
  `GET /api/v1/servers` authorizes in one batch. Paginate the endpoint before
  5,000-agent runs.
- **Reconnect storm** recovery is 12.0 s for 1,000 agents with full-jitter
  backoff; no thundering-herd errors were observed.
- Idle CPU at 1,000 agents (4.0 %) is dominated by long-poll turnover (every
  agent re-polls every 25 s) and stale-agent checks.

## Not yet measured

- Linux (Ubuntu) and Windows Server VMs, 24-hour agent idle run.
- 100 and 5,000 agents; PostgreSQL profile.
- 100 parallel deploys (Phase 3).
- CI regression gate (> 10 % fails the build): CI currently runs a 50-agent smoke run and uploads the result.

## Reproduce

```sh
cargo build --release -p forge-server -p forge-agent -p fleet-sim
target/release/fleet-sim --agents 50 --agents 1000 --idle-secs 60 --out bench.json
```

On Windows, run fleet-sim from an elevated prompt to get the real-agent
sample: `forge-agent enroll` locks `%ProgramData%\OrbynForge` to SYSTEM and
Administrators and fails with "Access is denied" otherwise. fleet-sim prints
the agent's error and skips the sample.
