#!/usr/bin/env python3
"""Turns monitor.py's samples.csv into averages, p95 and peaks per VM and
process, and writes a Markdown summary ready for BENCHMARKS.md.

CPU % is computed from the change in utime+stime between two consecutive
samples of the same PID, divided by elapsed time: 100 % = one full core. A PID
that restarts (new PID) starts a new series, so restarts don't create spikes.
Can be run while monitor.py is still writing.

Examples:
  python3 scripts/setup_testing/perf/summarize.py
  python3 scripts/setup_testing/perf/summarize.py --out-dir ~/forge-perf
"""
from __future__ import annotations

import argparse
import csv
import datetime as dt
import math
import sys
import tempfile
from collections import defaultdict
from pathlib import Path

DEFAULT_OUT = Path(tempfile.gettempdir()) / "forge-perf"


def ts(s: str) -> dt.datetime:
    return dt.datetime.fromisoformat(s.replace("Z", "+00:00"))


def stat(xs: list[float]) -> dict[str, float] | None:
    if not xs:
        return None
    s = sorted(xs)
    return {"avg": sum(s) / len(s), "min": s[0], "max": s[-1],
            "p95": s[min(len(s) - 1, math.floor(len(s) * 0.95))], "n": len(s)}


def g(st: dict[str, float] | None, key: str) -> float | None:
    return None if st is None else st[key]


def f(x: float | None, d: int = 1) -> str:
    return "—" if x is None else f"{x:.{d}f}"


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out-dir", type=Path, default=DEFAULT_OUT)
    a = ap.parse_args()
    out_dir: Path = a.out_dir.expanduser().resolve()
    src = out_dir / "samples.csv"
    with src.open(encoding="utf-8-sig", newline="") as fh:
        rows = list(csv.DictReader(fh))
    if not rows:
        sys.exit(f"no samples in {src}")

    first, last = ts(rows[0]["ts_utc"]), ts(rows[-1]["ts_utc"])
    span = last - first
    tck = {r["vm"]: float(r["v1"]) for r in rows if r["kind"] == "T"}

    proc_out: list[dict] = []
    node_out: list[dict] = []
    for vm in sorted({r["vm"] for r in rows}):
        r = [x for x in rows if x["vm"] == vm]
        hz = tck.get(vm) or 100.0

        # Per process name: RSS from every sample, CPU from deltas within the same PID.
        by_name: dict[str, list[dict]] = defaultdict(list)
        for x in r:
            if x["kind"] == "P":
                by_name[x["name"]].append(x)
        for name, group in by_name.items():
            rss = [float(x["v1"]) / 1024 for x in group]
            cpu: list[float] = []
            by_pid: dict[str, list[dict]] = defaultdict(list)
            for x in group:
                by_pid[x["pid"]].append(x)
            for series in by_pid.values():
                series.sort(key=lambda x: ts(x["ts_utc"]))
                for prev, cur in zip(series, series[1:]):
                    dts = (ts(cur["ts_utc"]) - ts(prev["ts_utc"])).total_seconds()
                    dj = float(cur["v2"]) - float(prev["v2"])
                    if dts > 0 and dj >= 0:
                        cpu.append(100 * dj / hz / dts)
            m, c = stat(rss), stat(cpu)
            proc_out.append({"VM": vm, "Process": name, "Pids": len(by_pid),
                             "RssAvg": g(m, "avg"), "RssP95": g(m, "p95"), "RssMax": g(m, "max"),
                             "CpuAvg": g(c, "avg"), "CpuP95": g(c, "p95"), "CpuMax": g(c, "max"),
                             "Samples": g(m, "n")})

        # Whole VM: memory used = MemTotal - MemAvailable; CPU from /proc/stat deltas.
        mem = [x for x in r if x["kind"] == "M"]
        used = [(float(x["v1"]) - float(x["v2"])) / 1024 for x in mem]
        total = [float(x["v1"]) / 1024 for x in mem]
        cs = sorted((x for x in r if x["kind"] == "C"), key=lambda x: ts(x["ts_utc"]))
        vcpu = []
        for prev, cur in zip(cs, cs[1:]):
            d_t = float(cur["v1"]) - float(prev["v1"])
            d_i = float(cur["v2"]) - float(prev["v2"])
            if d_t > 0:
                vcpu.append(100 * (d_t - d_i) / d_t)
        load = [float(x["name"]) for x in r if x["kind"] == "T"]
        u, t, v, l = stat(used), stat(total), stat(vcpu), stat(load)
        node_out.append({"VM": vm, "MemTotalAvg": g(t, "avg"), "UsedAvg": g(u, "avg"), "UsedMax": g(u, "max"),
                         "CpuAvg": g(v, "avg"), "CpuP95": g(v, "p95"), "LoadAvg": g(l, "avg"),
                         "Unreachable": sum(1 for x in r if x["kind"] == "E")})

    hours = math.floor(span.total_seconds() / 3600)
    minutes = int(span.total_seconds() // 60 % 60)
    vm_samples = sum(1 for x in rows if x["kind"] == "T")
    md = [
        f"## Idle footprint on the Hyper-V lab ({first:%Y-%m-%d} → {last:%Y-%m-%d})", "",
        f"Sampled every ~10 s for {hours} h {minutes} min ({vm_samples} VM samples) with "
        "`scripts/setup_testing/perf/monitor.py`; RSS from `/proc/<pid>/status` (VmRSS), "
        "CPU from utime+stime deltas (100 % = one core).", "",
        "### Forge processes", "",
        "| VM | Process | RSS avg (MiB) | RSS p95 | RSS max | CPU avg (%) | CPU p95 | CPU max | PIDs seen |",
        "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for p in sorted(proc_out, key=lambda p: (p["VM"], p["Process"])):
        md.append(f"| {p['VM']} | `{p['Process']}` | {f(p['RssAvg'])} | {f(p['RssP95'])} | {f(p['RssMax'])} | "
                  f"{f(p['CpuAvg'], 2)} | {f(p['CpuP95'], 2)} | {f(p['CpuMax'], 2)} | {p['Pids']} |")
    md += ["", "### Whole VM", "",
           "| VM | MemTotal avg (MiB) | Used avg (MiB) | Used max | CPU avg (%) | CPU p95 | Load1 avg | Missed samples |",
           "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"]
    for n in node_out:
        md.append(f"| {n['VM']} | {f(n['MemTotalAvg'], 0)} | {f(n['UsedAvg'], 0)} | {f(n['UsedMax'], 0)} | "
                  f"{f(n['CpuAvg'], 2)} | {f(n['CpuP95'], 2)} | {f(n['LoadAvg'], 2)} | {n['Unreachable']} |")
    md += ["", "### VM specs", "", "```text"]
    specs = out_dir / "specs.txt"
    if specs.exists():
        md += specs.read_text(encoding="utf-8").splitlines()[-7:]
    md.append("```")

    text = "\n".join(md) + "\n"
    out_file = out_dir / "summary.md"
    out_file.write_text(text, encoding="utf-8")
    for name, data in (("summary-processes.csv", proc_out), ("summary-vms.csv", node_out)):
        if data:
            with (out_dir / name).open("w", encoding="utf-8", newline="") as fh:
                w = csv.DictWriter(fh, fieldnames=list(data[0]))
                w.writeheader()
                w.writerows(data)
    print(text)
    print(f"Wrote {out_file}")


if __name__ == "__main__":
    main()
