#!/usr/bin/env python3
"""Live CPU and memory of Forge's own processes on this host.

Usage: python3 -I scripts/dev/forge_top.py [interval-seconds]   (Ctrl+C to stop)

CPU is shown two ways: % of the whole machine (all cores = 100 %) and % of
one core (one busy core = 100 %, like top). Memory is RSS in MB. Linux only;
standard library only.
"""
import os
import sys
import time

NAMES = ("forge-server", "forge-analytics-worker", "forge-agent", "forge-agent-helper", "clickhouse")
TICK = os.sysconf("SC_CLK_TCK")
CORES = os.cpu_count() or 1
PAGE_MB = os.sysconf("SC_PAGE_SIZE") / 1048576


def label(pid, comm):
    """Full program name plus what it is for, from the command line."""
    try:
        with open(f"/proc/{pid}/cmdline", "rb") as f:
            argv = [a.decode(errors="replace") for a in f.read().split(b"\0") if a]
    except OSError:
        return comm
    name = os.path.basename(argv[0]) if argv else comm
    if name == "forge-agent-helper" and len(argv) > 2 and argv[1] == "supervise":
        return f"{name} ({os.path.basename(argv[2]).removesuffix('.json')})"
    if name == "forge-agent":
        return f"{name} (node)"
    if name == "clickhouse":
        port = next((a.split("=", 1)[1] for a in argv if a.startswith("--http_port=")), None)
        return f"chDB engine (:{port})" if port else "chDB engine"
    return name


def procs():
    out = {}
    for pid in filter(str.isdigit, os.listdir("/proc")):
        try:
            with open(f"/proc/{pid}/stat") as f:
                stat = f.read()
            with open(f"/proc/{pid}/statm") as f:
                rss_pages = int(f.read().split()[1])
        except OSError:
            continue  # the process exited between listdir and open
        name = stat[stat.index("(") + 1:stat.rindex(")")]
        if not any(n[:15] == name for n in NAMES):
            continue  # /proc comm is cut at 15 characters
        name = label(pid, name)
        fields = stat[stat.rindex(")") + 2:].split()
        out[int(pid)] = (name, int(fields[11]) + int(fields[12]), rss_pages * PAGE_MB)
    return out


def main():
    interval = float(sys.argv[1]) if len(sys.argv) > 1 else 2.0
    before, t0 = procs(), time.monotonic()
    while True:
        time.sleep(interval)
        now, t1 = procs(), time.monotonic()
        wall = t1 - t0
        rows, tot_cpu, tot_mem = [], 0.0, 0.0
        for pid, (name, ticks, mb) in now.items():
            prev = before.get(pid)
            core_pct = (ticks - prev[1]) / TICK / wall * 100 if prev else 0.0
            rows.append((name, pid, core_pct, mb))
            tot_cpu += core_pct
            tot_mem += mb
        rows.sort(key=lambda r: (r[0], r[1]))
        print("\033[2J\033[H", end="")
        print(f"Forge processes · {CORES} cores · every {interval:g} s · {time.strftime('%H:%M:%S')}   (Ctrl+C to stop)\n")
        print(f"{'process':<38}{'pid':>9}{'% machine':>12}{'% one core':>13}{'RSS MB':>10}")
        for name, pid, core_pct, mb in rows:
            print(f"{name:<38}{pid:>9}{core_pct / CORES:>11.2f}%{core_pct:>12.1f}%{mb:>10.1f}")
        print("-" * 82)
        print(f"{'total':<38}{len(rows):>9}{tot_cpu / CORES:>11.2f}%{tot_cpu:>12.1f}%{tot_mem:>10.1f}", flush=True)
        before, t0 = now, t1


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        pass
