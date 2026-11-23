---
title: fleet-sim
description: Every fleet-sim flag, what each run measures, and the Markdown and JSON results it writes.
sidebar:
  order: 95
---

`fleet-sim` benchmarks the control plane with a fleet of simulated agents. It
starts a real `forge-server` process, enrolls N agents with real keys,
certificates and mTLS connections, and measures memory, CPU, API latency and
how fast the fleet reconnects after a crash. This page lists every flag and
output field. For the latest results and how to publish them, see
[Benchmarks](/docs/development/benchmarks/).

**Who runs it:** Forge contributors, on a development machine, as a normal
user. It needs no privileges and does not touch an existing installation.

## Synopsis

```
Usage: fleet-sim [OPTIONS]
```

On Windows the usage line says `fleet-sim.exe`.

## Options

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--agents <AGENTS>` | integer, repeatable | `50` and `1000` | No | Fleet sizes to run, one run each, in order. Giving the flag replaces both defaults | `--agents 50 --agents 1000` |
| `--idle-secs <IDLE_SECS>` | integer, seconds | `60` | No | How long to sample the idle fleet, once per second. At least 2 samples are taken | `--idle-secs 120` |
| `--hold-secs <HOLD_SECS>` | integer, seconds | `25` | No | Long-poll hold the server uses (`heartbeat_hold_secs`). The production default is also `25` | `--hold-secs 25` |
| `--server-bin <SERVER_BIN>` | path | `forge-server` next to `fleet-sim` | No | The `forge-server` binary to start | `--server-bin target/release/forge-server` |
| `--agent-bin <AGENT_BIN>` | path | `forge-agent` next to `fleet-sim` | No | The `forge-agent` binary for the real agent process | `--agent-bin target/release/forge-agent` |
| `--no-agent-process` | flag | off | No | Skips the real `forge-agent` process and its measurement | `--no-agent-process` |
| `--no-names` | flag | off | No | Writes `[names] enabled = false` into the server config, so the fleet-wide local-names sync does not run. Use it to measure that feature's cost | `--no-names` |
| `--out <OUT>` | path | none | No | Also write the results as pretty-printed JSON to this file | `--out bench.json` |
| `--target <TARGET>` | URL | none | No | Soak mode: the API base URL of a `forge-server` that is already running. See [Soak mode](#soak-mode) | `--target http://127.0.0.1:56130` |
| `--target-agent-url <TARGET_AGENT_URL>` | URL | `https://127.0.0.1:7443` | No | Soak mode: that server's agent endpoint | `--target-agent-url https://127.0.0.1:56131` |
| `--login <LOGIN>` | string | `admin` | No | Soak mode: the user that creates enrollment tokens. MFA must be off for it | `--login admin` |
| `--password-file <PASSWORD_FILE>` | path | none | In soak mode | Soak mode: file holding that user's password | `--password-file admin-password.txt` |
| `--state-dir <STATE_DIR>` | path | `fleet-sim-nodes` | No | Soak mode: where node identities are kept. Nodes found there are reused, so a later, larger run enrolls only the new ones | `--state-dir /tmp/nodes` |
| `--duration-secs <DURATION_SECS>` | integer, seconds | `300` | No | Soak mode: how long the fleet runs | `--duration-secs 600` |
| `--report-secs <REPORT_SECS>` | integer, seconds | `30` | No | Soak mode: seconds between report lines on standard error | `--report-secs 10` |
| `--log-lines-per-sec <LOG_LINES_PER_SEC>` | float | `0` | No | Soak mode: container log lines each node writes per second | `--log-lines-per-sec 20` |
| `--containers <CONTAINERS>` | integer | `0` | No | Soak mode: containers per node. Each sends a resource sample once a minute and is a source of log lines | `--containers 5` |
| `--logging-containers <LOGGING_CONTAINERS>` | integer | `0` | No | Soak mode: how many of `--containers` write the log lines; the rest are silent but still send their samples. `0` means all | `--logging-containers 10` |
| `--worker-url <WORKER_URL>` | URL | the URL the server names | No | Soak mode: the analytics worker's agent listener the nodes upload their spool files to. Without it, nodes use the URL from enrollment and then the one in each heartbeat response | `--worker-url https://127.0.0.1:7445` |
| `--server-pid <SERVER_PID>` | integer | none | No | Soak mode: the `forge-server` process id. Every report then shows its resident memory and open file descriptors. Linux only; elsewhere the values are `null` | `--server-pid 4242` |
| `--max-server-rss-mb <MAX_SERVER_RSS_MB>` | float, MB | none | No | Soak mode: exits with an error after printing the totals when the server's RSS was above this in any report. Needs `--server-pid` on Linux; without a sample the run fails | `--max-server-rss-mb 512` |
| `--settle-secs <SETTLE_SECS>` | integer, seconds | `30` | No | Soak mode: how long to wait after the load stops before the last server sample and the count of spool files left. Uploads keep running meanwhile | `--settle-secs 60` |
| `-h`, `--help` | flag | | No | Prints help | `fleet-sim --help` |

`fleet-sim` reads no environment variables itself. The `forge-server` it
starts inherits its environment, so `FORGE_DATABASE_URL` runs the benchmark
against that database (for example an empty Postgres database) instead of a
new SQLite file. It sets `RUST_LOG=warn` for the processes it starts.

## What each run does

For each value of `--agents`, in a new temporary directory:

1. Writes a `forge.toml` that listens on two free ports on `127.0.0.1`, with
   SQLite and `heartbeat_hold_secs` from `--hold-secs`.
2. Runs `forge-server init` with a throwaway admin, starts
   `forge-server serve` and waits up to 10 s for `GET /api/health`.
3. Signs in (solving the login challenge if the server asks), then enrolls N
   agents, 64 at a time. Each agent gets its own token, key, certificate and
   HTTP/2 mTLS connection. All agents run inside the `fleet-sim` process.
   They allow only the `ping` and `collect_facts` commands and refuse every
   other command locally, so they never change the benchmark host (such as
   its hosts file). The server still builds, signs and delivers each command
   and receives the refusal as its result.
4. Waits up to 300 s until every agent is online.
5. On the first fleet size only, unless `--no-agent-process`: enrolls and
   starts one real `forge-agent` process and waits up to 60 s for it. On
   Windows this step needs an elevated prompt (see [Exit codes](#exit-codes)).
6. Samples the RSS and CPU of `forge-server` (and the real agent) once per
   second for `--idle-secs`.
7. Sends 200 sequential `GET /api/v1/servers` requests and records p50 and
   p99 latency.
8. Records the size of `forge.db`.
9. **Reconnect storm:** kills the server, waits 2 s, starts it again, and
   times until every agent has sent a heartbeat after the restart (up to
   300 s).

The temporary directory, database and admin are deleted when the run ends.

## Soak mode

With `--target`, `fleet-sim` starts no server. It signs in to the running
server, enrolls the nodes it does not have in `--state-dir` yet, and runs
exactly one `--agents` value for `--duration-secs`. Use it for capacity
tests: the reports in `scripts/dev/benchmarks/` in the repository use it.

Soak nodes are lighter than the in-process agents of a normal run. Each
speaks the agent protocol over its own mTLS HTTP/2 connection but builds its
telemetry instead of reading the host. Like `forge-agent`, a node sends no
analytics data in heartbeats. It writes rows to its own disk spool in
`<state-dir>/<i>/spool` and uploads the sealed files over mTLS to the
analytics worker's agent listener:

- on every heartbeat, the container log lines written since the last one,
  at `--log-lines-per-sec`, spread over the first `--logging-containers`
  containers (all of them by default);
- once a minute, the closed node minute, ten top processes and one sample
  per container (`--containers`);
- the buffer is sealed into a file after each heartbeat's rows;
- the heartbeat carries the live resource sample (without containers or
  processes) and the spool's status;
- every command is refused, so nothing on the host changes.

A line counts as acknowledged when its file leaves the spool by an upload,
and as dropped when the spool refuses it or drops its file (size cap, age,
or rejected by the worker).

:::note
Each node's spool counts against the free space of the disk that holds
`--state-dir`. A spool stops taking rows when that disk has less than
256 MiB free.
:::

With `--server-pid`, `fleet-sim` samples the server's RSS and open file
descriptors before the fleet starts (one JSON line on standard error), in
every report, and `--settle-secs` after the load stops. Compare the
baseline and the after values to see whether memory, descriptors and spool
files return to where they started.

Every `--report-secs`, a JSON line goes to standard error; at the end, the
totals go to standard output (and `--out`):

| Field | Type, unit | Description |
| --- | --- | --- |
| `elapsed_secs` | float, s | Time since the fleet started |
| `agents` | integer | Fleet size |
| `heartbeats`, `heartbeats_failed` | integer | Heartbeats answered, and failed or timed out |
| `heartbeats_early` | integer | Answered before the hold with commands; not timed |
| `hb_p50_ms`, `hb_p95_ms`, `hb_p99_ms`, `hb_max_ms` | float, ms | Heartbeat round trip minus `--hold-secs`: the time the server spent on the heartbeat, plus the network |
| `lines_generated`, `lines_sent`, `lines_acked`, `lines_dropped` | integer | Container lines written, appended to the spools, in files the worker acknowledged, and refused or in dropped files |
| `container_samples` | integer | Container sample rows spooled |
| `files_uploaded`, `files_dropped` | integer | Spool files the worker acknowledged, and files the spools dropped |
| `spool_files_waiting`, `spool_bytes_waiting` | integer, bytes | Sealed files and their bytes waiting in all spools at report time |
| `offered_per_sec`, `acked_per_sec` | float | Lines written and acknowledged per second over the interval |
| `server_rss_mb`, `server_fds` | float MB, integer, or `null` | The server's resident memory and open descriptors at report time. In the totals, the values after settling. `null` without `--server-pid` or off Linux |
| `server_rss_mb_peak` | float, MB | Totals only: the highest RSS of any report and the after sample |
| `server_rss_mb_baseline`, `server_fds_baseline` | float MB, integer | Totals only: sampled before the fleet started |
| `server_rss_mb_after`, `server_fds_after` | float MB, integer | Totals only: sampled `--settle-secs` after the load stopped |
| `spool_files_after` | integer | Totals only: sealed files left in all spools after settling |

The totals count the whole run; `lines_acked` also includes files uploaded
while settling. Fields that need `--server-pid` are left out of the totals
when there is no sample.

```sh
target/release/fleet-sim --target http://127.0.0.1:56130 --target-agent-url https://127.0.0.1:56131 \
  --password-file <data-dir>/admin-password.txt --state-dir <scratch>/nodes \
  --agents 500 --duration-secs 300 --log-lines-per-sec 20 --containers 5 \
  --server-pid $(pgrep -x forge-server) --max-server-rss-mb 512
```

## Output

| Stream | Content |
| --- | --- |
| standard error | `== <N> agents ==` when a run starts, then that run's result as one JSON line |
| standard output | A Markdown table with one row per run, and one line for the real agent process |
| `--out` file | All runs as a pretty-printed JSON array |

### Result fields

| Field | Type, unit | Markdown column | Description |
| --- | --- | --- | --- |
| `agents` | integer | Agents | Fleet size of the run |
| `enroll_secs` | float, s | Enroll (s) | Time to enroll all N agents |
| `all_online_secs` | float, s | All online (s) | Time from the start of the agents until all are online |
| `server_idle.rss_mb_avg` | float, MB | CP RSS avg | Control-plane resident memory, average over the idle samples |
| `server_idle.rss_mb_max` | float, MB | CP RSS max | Control-plane resident memory, highest sample |
| `server_idle.cpu_pct_avg` | float, % of one core | CP CPU avg (% core) | Control-plane CPU, average over the idle samples |
| `api_p50_ms` | float, ms | API p50 | Median latency of `GET /api/v1/servers` |
| `api_p99_ms` | float, ms | API p99 | 99th percentile latency of `GET /api/v1/servers` |
| `reconnect_storm_secs` | float, s | Reconnect storm (s) | Time from the restart until every agent has reconnected |
| `db_size_mb` | float, MB | DB (MB) | Size of `forge.db` after the agents enrolled |
| `agent_process` | object or `null` | separate line | `rss_mb_avg`, `rss_mb_max` and `cpu_pct_avg` of the real `forge-agent` process. `null` on later runs and with `--no-agent-process` |

MB means MiB (1,048,576 bytes).

## Examples

Build the binaries and run the standard benchmark:

```sh
cargo build --release -p forge-server -p forge-agent -p fleet-sim
target/release/fleet-sim --agents 50 --agents 1000 --out bench.json
```

A quick check with a small fleet and a short sample:

```sh
target/release/fleet-sim --agents 10 --idle-secs 5 --no-agent-process
```

Compare SQLite and Postgres with `scripts/dev/bench.py` (Linux and Windows). It
builds the binaries with the `perf` profile, runs every fleet size on SQLite,
then on Postgres with a new empty database per size, and drops that database
at the end. Results go to `target/bench/` (one JSON file per run and
`summary.md`):

```sh
python3 scripts/dev/bench.py --postgres-url postgres://<user>:<password>@localhost:5432 --agents 50 1000
```

| Parameter | Default | Meaning |
| --- | --- | --- |
| `--agents` | `50 1000` | Fleet sizes |
| `--postgres-url` | none (SQLite only) | Admin connection URL without a database name |
| `--postgres-db` | `forge_bench` | Database created and dropped for each run. Do not point it at a database you use |
| `--psql` | `psql` on `PATH`; on Windows the newest `C:\Program Files\PostgreSQL\<n>\bin\psql.exe` | Path of `psql` |
| `--out-dir` | `target/bench` | Where results are written |
| `--no-names` | off | Passes `--no-names` |
| `--skip-sqlite` | off | Runs Postgres only |
| `--no-build` | off | Uses the binaries already in `target/perf` |

With Postgres, the **DB (MB)** column is not meaningful; `summary.md` records
`pg_database_size` after each run instead.

Use binaries from another directory:

```sh
fleet-sim --server-bin <path>/forge-server --agent-bin <path>/forge-agent --agents 100
```

## Sample output

The help text is real. The results below were not produced for this page:
they are a run recorded in `BENCHMARKS.md` (Windows 11 workstation, SQLite,
release build), shown in the format the code prints.

```text
Simulate a fleet of Orbyn Forge agents

Usage: fleet-sim [OPTIONS]

Options:
      --agents <AGENTS>          Fleet sizes to run, e.g. `--agents 50 --agents 1000` [default: 50 1000]
      --idle-secs <IDLE_SECS>    Seconds to sample the idle fleet [default: 60]
      --hold-secs <HOLD_SECS>    Long-poll hold the server uses (production default 25) [default: 25]
      --server-bin <SERVER_BIN>  Path of forge-server (default: next to this binary)
      --agent-bin <AGENT_BIN>    Path of forge-agent (default: next to this binary). Use `--no-agent-process` to skip
      --no-agent-process
      --no-names                 Turn off the fleet-wide local-names sync (`[names] enabled = false`) to measure its cost
      --out <OUT>                Write results as JSON here
  -h, --help                     Print help
```

Standard output:

```markdown
| Agents | Enroll (s) | All online (s) | CP RSS avg / max (MB) | CP CPU avg (% core) | API p50 / p99 (ms) | Reconnect storm (s) | DB (MB) |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 50 | 0.5 | 0.3 | 24.0 / 24.0 | 0.27 | 15.5 / 38.4 | 0.0 | 0.44 |
| 1000 | 19.1 | 1.1 | 72.7 / 72.8 | 4.36 | 644.3 / 1720.5 | 8.9 | 2.20 |

Real `forge-agent` process, idle: RSS avg 17.1 MB (max 17.2 MB), CPU avg 0.22 % of one core.
```

The `--out` file for the same run, with the values rounded as in the table:

```json
[
  {
    "agents": 50,
    "enroll_secs": 0.5,
    "all_online_secs": 0.3,
    "server_idle": { "rss_mb_avg": 24.0, "rss_mb_max": 24.0, "cpu_pct_avg": 0.27 },
    "api_p50_ms": 15.5,
    "api_p99_ms": 38.4,
    "reconnect_storm_secs": 0.0,
    "db_size_mb": 0.44,
    "agent_process": { "rss_mb_avg": 17.1, "rss_mb_max": 17.2, "cpu_pct_avg": 0.22 }
  },
  {
    "agents": 1000,
    "enroll_secs": 19.1,
    "all_online_secs": 1.1,
    "server_idle": { "rss_mb_avg": 72.7, "rss_mb_max": 72.8, "cpu_pct_avg": 4.36 },
    "api_p50_ms": 644.3,
    "api_p99_ms": 1720.5,
    "reconnect_storm_secs": 8.9,
    "db_size_mb": 2.2,
    "agent_process": null
  }
]
```

The real file has full precision.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Every run finished |
| `1` | A run failed. `fleet-sim` prints `Error: <message>` and stops |
| `2` | Usage error: unknown or invalid flag |

| Error | Cause | Fix |
| --- | --- | --- |
| `No such file or directory (os error 2)` (Linux) or `program not found` (Windows) | The `forge-server` binary was not found | Build it next to `fleet-sim`, or pass `--server-bin` |
| `forge-server init failed` | `forge-server init` exited with an error, for example because the binary is from a different version | Rebuild all three binaries from the same tree |
| `server did not become healthy` | The server did not answer `/api/health` within 10 s | Run it by hand with the same version to see why |
| `only <n> of <N> agents online after 300s` | The machine cannot hold that many connections, or ran out of file handles | Use a smaller `--agents`, or raise the open-file limit (`ulimit -n`) |
| `only <n> of <N> agents reconnected` | The fleet did not come back within 300 s after the restart | As above |
| `warning: real agent enrollment failed (exit code: <n>); skipping agent process sample`, followed by the agent's error | `forge-agent` failed to enroll. The run goes on without it. The next lines show the agent's own error | Read the agent's error and fix that cause |
| The same warning, with `Error: Access is denied. (os error 5)` (Windows) | `forge-agent enroll` locks `%ProgramData%\OrbynForge` to SYSTEM and Administrators, which needs Administrator. fleet-sim prints a hint line | Run `fleet-sim` from an elevated prompt |
| The same warning, with `program not found` (Windows) or `os error 2` (Linux) | The `forge-agent` binary was not found | Build it next to `fleet-sim`, or pass `--agent-bin` |

:::note
All simulated agents run in one process on the same host as the control
plane, so results show control-plane cost, not network effects. Compare runs
only on the same machine and build profile.
:::

## See also

- [Benchmarks](/docs/development/benchmarks/): current results and targets.
- [forge-agent](/docs/reference/cli/forge-agent/): the agent that `fleet-sim`
  simulates and starts once as a real process.
- [Configuration](/docs/reference/configuration/): `heartbeat_hold_secs` and the
  other server keys.
