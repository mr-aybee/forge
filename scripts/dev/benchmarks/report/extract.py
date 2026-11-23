#!/usr/bin/env python3
"""Pull the figures the HTML report needs out of capacity-bench result dirs.

Usage: python3 -I extract.py <results-root> > data.json
<results-root> holds bench/, bench2/, bench3/ and bench-duck/ as written by
scripts/dev/capacity-bench. Raw results are not kept in the repository; the
compact data.json is.
"""
import json
import pathlib
import sys

ROOT = pathlib.Path(sys.argv[1])
STEP = 15.0  # seconds between kept samples in time series


def load(path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


def phase(run, name, label, **extra):
    d = ROOT / run / "results" / name
    s = load(d / "summary.json")
    if not s:
        return None
    api = s.get("api_ms", {})
    out = {
        "id": name, "label": label,
        "offered": s.get("offered_per_sec"), "acked": s.get("acked_per_sec"),
        "stored_per_sec": s.get("stored_per_sec"), "drain": s.get("drain_secs"),
        "server_cpu": s.get("server_cpu_core_pct", {}).get("avg"),
        "server_rss": s.get("server_rss_mb", {}).get("max"),
        "worker_cpu": s.get("worker_cpu_core_pct", {}).get("avg"),
        "worker_cpu_max": s.get("worker_cpu_core_pct", {}).get("max"),
        "worker_rss": s.get("worker_rss_mb", {}).get("max"),
        "engine_cpu": s.get("engine_cpu_core_pct", {}).get("avg"),
        "engine_cpu_max": s.get("engine_cpu_core_pct", {}).get("max"),
        "search_p50": api.get("logs_search", {}).get("p50"),
        "search_p95": api.get("logs_search", {}).get("p95"),
        "hist_p95": api.get("logs_histogram", {}).get("p95"),
        "lines_acked": s.get("totals", {}).get("lines_acked"),
        "dropped": s.get("totals", {}).get("lines_dropped"),
    }
    out.update(extra)
    return out


def series(run, name, keys, agent=False):
    d = ROOT / run / "results" / name
    rows = []
    for f, prefix in [("sample.jsonl", "")] + ([("agent.jsonl", "a:")] if agent else []):
        p = d / f
        if not p.exists():
            continue
        for line in p.read_text().splitlines():
            try:
                rows.append((prefix, json.loads(line)))
            except ValueError:
                continue
    if not rows:
        return None
    t0 = min(r["t"] for _, r in rows)
    ready = load(d / "ready_t")
    out, last = {}, {}
    for prefix, r in sorted(rows, key=lambda x: x[1]["t"]):
        t = r["t"] - t0
        for k in keys:
            src = k[2:] if k.startswith("a:") else k
            if (k.startswith("a:")) != (prefix == "a:") or r.get(src) is None:
                continue
            if t - last.get(k, -STEP) >= STEP:
                out.setdefault(k, []).append([round(t), round(float(r[src]), 1)])
                last[k] = t
    return {"t_ready": round(ready - t0) if isinstance(ready, (int, float)) else None, "series": out}


CPU_MEM = ["server_cpu_core_pct", "server_rss_mb", "worker_cpu_core_pct", "worker_rss_mb",
           "engine_cpu_core_pct", "engine_anon_mb", "rows"]
data = {
    "capped": [p for p in [
        phase("bench", "p50", "50"), phase("bench", "p250", "250"),
        phase("bench", "p500c", "500"), phase("bench", "p1000", "1,000"),
        phase("bench", "p1500", "1,500"), phase("bench", "p2000b", "2,000"),
    ] if p],
    "overload": phase("bench", "p500x3", "500 at 3×"),
    "uncapped50": phase("bench2", "p50", "chDB, 50 × 20"),
    "uncapped50_ts": series("bench2", "p50", CPU_MEM),
    "agent50": phase("bench3", "main", "chDB + real agent"),
    "agent_ts": series("bench3", "main", CPU_MEM + [
        "a:agent_cpu_core_pct", "a:agent_VmRSS_mb", "a:helper_cpu_core_pct", "a:helper_VmRSS_mb"], agent=True),
    "duck": [p for p in [
        phase("bench-duck", "d_light", "5 nodes, light"),
        phase("bench-duck", "d_p50", "50 × 20 (10/s)"),
        phase("bench-duck", "d_p100", "100 × 20 (10/s)"),
        phase("bench-duck", "d_p100x20", "100 × 20 (20/s)"),
        phase("bench-duck", "d_p100x40", "100 × 20 (40/s)"),
        phase("bench-duck", "d_p100x20nr", "100 × 20 (20/s), no readers"),
        phase("bench-duck", "d_p50_tuned", "50 × 20 (10/s), tuned"),
        phase("bench-duck", "d_p100_tuned", "100 × 20 (10/s), tuned"),
    ] if p],
    "chdb100": phase("bench-duck", "c_p100", "chDB uncapped, 100 × 20 (10/s)"),
    "duck50_ts": series("bench-duck", "d_p50", CPU_MEM),
}
def replay(run, name, store):
    """Every 5 s sample of one run, aligned so t=0 is the start of the load.

    Store CPU/memory: chDB = worker + engine process; DuckDB = the worker
    (DuckDB runs inside it). Memory excludes chDB's mapped binary (~520 MB),
    which the page can add on top.
    """
    d = ROOT / run / "results" / name
    p = d / "sample.jsonl"
    if not p.exists():
        return None
    rows = []
    for line in p.read_text().splitlines():
        try:
            rows.append(json.loads(line))
        except ValueError:
            continue
    if not rows:
        return None
    ready = load(d / "ready_t")
    t0 = ready if isinstance(ready, (int, float)) else rows[0]["t"]
    summ = load(d / "summary.json") or {}
    sim = load(d / "sim-total.json") or {}
    out = []
    for r in rows:
        if r.get("server_cpu_core_pct") is None:
            continue  # the first sample has no CPU delta yet
        w_cpu, w_mem = r.get("worker_cpu_core_pct") or 0, r.get("worker_rss_mb") or 0
        if store == "chdb":
            cpu = w_cpu + (r.get("engine_cpu_core_pct") or 0)
            mem = w_mem + (r.get("engine_anon_mb") or 0)
        else:
            cpu, mem = w_cpu, w_mem
        out.append([round(r["t"] - t0, 1), round(cpu / 100, 3), round(mem, 1),
                    round((r.get("server_cpu_core_pct") or 0) / 100, 3), round(r.get("server_rss_mb") or 0, 1),
                    r.get("spool_files") or 0])
    return {"run": f"{run}/{name}", "load_secs": round(sim.get("elapsed_secs") or 240),
            "offered": summ.get("offered_per_sec"), "acked": summ.get("acked_per_sec"),
            "drain": summ.get("drain_secs"), "search_p95": (summ.get("api_ms", {}).get("logs_search") or {}).get("p95"),
            "points": out}


REPLAY = [
    # id, label, description, [(store, run, phase, note)]
    ("light", "Lightweight · 5 servers", "5 servers × 20 services, 5 of them logging 2 lines/s (about 50 lines/s in total). A busy lightweight install.",
     [("duck", "bench-duck", "d_light", "")]),
    ("p50", "50 servers · 5,000 lines/s", "50 servers × 20 services, 10 of them logging 10 lines/s. The same load on both stores, nothing capped.",
     [("chdb", "bench2", "p50", "uncapped"), ("duck", "bench-duck", "d_p50", "")]),
    ("p100", "100 servers · 10,000 lines/s", "100 servers × 20 services, 10 logging 10 lines/s. The same load on both stores, nothing capped.",
     [("chdb", "bench-duck", "c_p100", "uncapped"), ("duck", "bench-duck", "d_p100", "")]),
    ("p50t", "50 servers · DuckDB tuned", "The 50-server load again after tuning DuckDB (group commit, 32 upload slots, reads capped at DuckDB's threads), against chDB uncapped.",
     [("chdb", "bench2", "p50", "uncapped"), ("duck", "bench-duck", "d_p50_tuned", "tuned")]),
    ("p100t", "100 servers · DuckDB tuned", "The 100-server load again after tuning DuckDB, against chDB uncapped at the same load.",
     [("chdb", "bench-duck", "c_p100", "uncapped"), ("duck", "bench-duck", "d_p100_tuned", "tuned")]),
    ("p100x20", "100 servers · 20,000 lines/s", "100 servers × 20 services, 10 logging 20 lines/s. chDB is shown at the same rate from the 2-core test (1,000 servers × 20 lines/s).",
     [("chdb", "bench", "p1000", "capped at 2 cores, 1,000 servers × 20/s"), ("duck", "bench-duck", "d_p100x20", "")]),
    ("p100x40", "100 servers · 40,000 lines/s", "100 servers × 20 services, 10 logging 40 lines/s. chDB is shown at the same rate from the 2-core test (2,000 servers × 20 lines/s).",
     [("chdb", "bench", "p2000b", "capped at 2 cores, 2,000 servers × 20/s"), ("duck", "bench-duck", "d_p100x40", "")]),
]
data["replay"] = []
for wid, label, desc, sources in REPLAY:
    stores = {}
    for store, run, name, note in sources:
        r = replay(run, name, store)
        if r and r["points"]:
            r["note"] = note
            stores[store] = r
    if stores:
        data["replay"].append({"id": wid, "label": label, "desc": desc, "stores": stores})
json.dump(data, sys.stdout, indent=1)
