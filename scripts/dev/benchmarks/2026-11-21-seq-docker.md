# Seq in Docker at 2,000 events/s (comparison)

[← All benchmarks](README.md)

| | |
| --- | --- |
| Date | 2026-11-21 |
| Commit | `d760f1a` |
| System | 16 cores (Xeon E5-2620 v2), 64 GB RAM, Linux 6.8, Docker |
| Flow | CLEF over HTTP into Seq (no Forge) |
| Store | Seq `datalust/seq:latest` (24.04 label), default settings |
| Load | 2,000 events/s for 180 s, batches of 500 |
| Verdict | Seq: about 17 % of one core steady, about 140 MiB. Lighter than the old Forge pipeline (embedded chDB before the quiet config) at this volume |

How much CPU and memory Seq uses under a steady structured-log load, for
comparison with Forge's log pipeline. A one-machine dev measurement, not
a capacity test.

## Setup

| Item | Value |
| --- | --- |
| Host | 16 cores (Xeon E5-2620 v2), 64 GB RAM, Linux 6.8 |
| Seq | `datalust/seq:latest` (version 24.04 label, image built 2026-10-29), Docker, no authentication, default settings |
| Load | 2,000 CLEF events/s for 180 s (360,000 events), batches of 500 to `/ingest/clef` |
| Event shape | 5 message templates, levels mostly Information plus Debug/Warning/Error, about 15 properties, a 50–400 byte payload property |
| Sampling | `docker stats` every 5 s: 20 s idle, during the load, 60 s after |

CPU figures are per core, as in `docker stats` and `top`: 100% is one full
core and the host has 1600%.

## Run it again

```sh
# Start Seq on 127.0.0.1:5399
sg docker -c "docker run -d --name seq-bench -e ACCEPT_EULA=Y \
  -e SEQ_FIRSTRUN_NOAUTHENTICATION=true -p 127.0.0.1:5399:80 datalust/seq:latest"

# Idle, load (2,000 events/s for 180 s) and after, sampled every 5 s
bash scripts/dev/seq-bench/run.sh | tee /tmp/seq-stats.txt

# Change the rate or duration: python3 -I scripts/dev/seq-bench/load.py <events/s> <seconds>

# Remove it
sg docker -c "docker rm -f seq-bench"
```

`load.py` uses only the Python standard library.

## Results

| Phase | CPU (% of one core) | Memory |
| --- | --- | --- |
| Start-up | 116% for one sample | 41 MiB |
| Idle | 1–8% | 78 MiB |
| Load, first 30 s | 14–111% (spikes during start of indexing) | 84 → 142 MiB |
| Load, steady | 15–40%, settling at about 17% | 121–147 MiB |
| After the load | 1–3% | 142 MiB, flat |

- Delivered rate: 1,990 events/s (360,000 events in 181 s).
- Peak memory: 147 MiB. Peak CPU: 111% of one core (about 7% of the host).
- Data on disk after the run: 81 MiB (`/data`).

## Compared with Forge

Forge's numbers come from the dev environment in `MOP-dev-env.md`, at its
normal load (three nodes' heartbeats, container logs, the two portals). That
load is lighter than this test.

| | CPU (% of one core) | Memory |
| --- | --- | --- |
| Seq, steady at 2,000 events/s | about 17% (peaks 111%) | about 140 MiB |
| Embedded ClickHouse, old config (all system logs on) | about 23% | 755 MiB RSS, of which 196 MiB its own; the rest is the mapped 550 MiB binary |
| Embedded ClickHouse, quiet config (system logs off), idle | about 12% | about 92 MiB own memory, plus the mapped binary |
| `forge-server` (debug build) | about 2% | about 155 MiB |

What this shows:

- At this volume Seq is lighter than Forge's pipeline. Its memory is about
  the same as `forge-server` alone, and the embedded engine adds its own
  memory and a CPU floor even when idle.
- Most of the engine's RSS is the ClickHouse binary mapped into memory. That
  memory is shared and can be reclaimed, so RSS overstates the real cost.
- Seq caches recent events in memory and its use grows with stored volume;
  81 MiB of data is small. This test does not show how either system behaves
  with weeks of logs or many gigabytes.

## Not yet measured

- Forge's pipeline under the same 2,000 events/s load (`/api/events/raw`
  with a log ingest key), after the quiet engine config and the CPU and
  memory limits are deployed.
- Both systems at a larger stored volume (gigabytes) and with searches
  running during ingest.
