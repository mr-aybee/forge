"""Summary of one phase directory: steady state after a 60 s warm-up, and
the drain after the load stopped.

Usage: python3 -I summarize.py <phase_dir> <load_secs> [warmup_secs]
"""
import collections, json, os, re, sys

D = sys.argv[1]
SECS = float(sys.argv[2])
WARM = float(sys.argv[3]) if len(sys.argv) > 3 else 60.0


def lines(name):
    p = os.path.join(D, name)
    if not os.path.exists(p):
        return []
    out = []
    for l in open(p, errors="replace"):
        try:
            v = json.loads(l)
            if isinstance(v, dict):
                out.append(v)
        except ValueError:
            pass
    return out


def pct(v, p):
    v = sorted(v)
    return round(v[min(len(v) - 1, int(round((len(v) - 1) * p)))], 1) if v else None


def stats(v):
    v = [x for x in v if isinstance(x, (int, float))]
    return {"avg": round(sum(v) / len(v), 1), "max": round(max(v), 1)} if v else None


def num(name):
    try:
        return int(open(os.path.join(D, name)).read().strip())
    except (OSError, ValueError):
        return None


ready = float(open(os.path.join(D, "ready_t")).read())
stop = ready + SECS
res = {"phase": os.path.basename(D)}
sim = [r for r in lines("sim.jsonl") if "hb_p50_ms" in r]
steady = [r for r in sim if r["elapsed_secs"] > WARM]
tot = json.load(open(os.path.join(D, "sim-total.json"))) if os.path.exists(os.path.join(D, "sim-total.json")) else {}
res["agents"] = tot.get("agents")
if steady:
    secs = 10 * len(steady)
    res["hb_ms_p95_max"] = round(max(r["hb_p95_ms"] for r in steady), 1)
    res["hb_ms_p50_median"] = pct([r["hb_p50_ms"] for r in steady], 0.5)
    res["offered_per_sec"] = round(sum(r["lines_generated"] for r in steady) / secs, 1)
    res["acked_per_sec"] = round(sum(r["lines_acked"] for r in steady) / secs, 1)
    res["spool_files_waiting_max"] = max(r["spool_files_waiting"] for r in sim)
    res["spool_mb_waiting_max"] = round(max(r["spool_bytes_waiting"] for r in sim) / 2**20, 1)
res["totals"] = {k: tot.get(k) for k in (
    "heartbeats", "heartbeats_failed", "lines_generated", "lines_sent", "lines_acked", "lines_dropped",
    "files_uploaded", "files_dropped", "uploads_busy", "uploads_failed", "spool_files_after",
    "server_rss_mb_baseline", "server_rss_mb_peak", "server_rss_mb_after", "server_fds_baseline",
    "server_fds_after")}
rb, ra = num("rows_before"), num("rows_after")
res["rows_stored"] = (ra - rb) if rb is not None and ra is not None else None
smp = lines("sample.jsonl")
load = [r for r in smp if ready + WARM <= r["t"] <= stop]
for p in ("server", "worker", "engine", "sim"):
    for k in ("cpu_core_pct", "rss_mb", "anon_mb", "file_mb", "fds", "threads"):
        s = stats([r.get(f"{p}_{k}") for r in load])
        if s:
            res[f"{p}_{k}"] = s
res["engine_cpus"] = next((r["engine_cpus"] for r in smp if r.get("engine_cpus")), None)
res["spool_files_disk"] = stats([r.get("spool_files") for r in smp if r["t"] >= ready])
# Drain: first sample after the load stopped with no ready files on disk.
after = [r for r in smp if r["t"] >= stop]
drained = next((r for r in after if r.get("spool_files") == 0), None)
res["drain_secs"] = round(drained["t"] - stop, 0) if drained else None
res["backlog_at_stop"] = after[0].get("spool_files") if after else None
# Store rate from the engine's row count during steady load.
rr = [r for r in load if r.get("rows") is not None]
if len(rr) >= 2:
    res["stored_per_sec"] = round((rr[-1]["rows"] - rr[0]["rows"]) / (rr[-1]["t"] - rr[0]["t"]), 1)
# Disk: growth of the analytics data over the phase, and write throughput.
disk = [r for r in smp if r.get("analytics_mb") is not None]
if disk:
    res["analytics_mb"] = {"start": disk[0]["analytics_mb"], "end": disk[-1]["analytics_mb"]}
for p in ("worker", "engine"):
    wb = [r for r in load if r.get(f"{p}_write_bytes") is not None]
    if len(wb) >= 2 and wb[-1]["t"] > wb[0]["t"]:
        res[f"{p}_write_mb_s"] = round(
            (wb[-1][f"{p}_write_bytes"] - wb[0][f"{p}_write_bytes"]) / 2**20 / (wb[-1]["t"] - wb[0]["t"]), 2)
rd = [r for r in lines("readers.jsonl") if r["t"] >= ready + WARM]
api = collections.defaultdict(list)
errs = collections.Counter()
for r in rd:
    api[r["ep"]].append(r["ms"])
    if r["status"] != 200:
        errs[f'{r["ep"]}:{r["status"]}'] += 1
res["api_ms"] = {ep: {"n": len(v), "p50": pct(v, .5), "p95": pct(v, .95)} for ep, v in api.items()}
res["api_errors"] = dict(errs)
log = open(os.path.join(D, "server.log"), errors="replace").read() if os.path.exists(os.path.join(D, "server.log")) else ""
plain = re.sub(r"\x1b\[[0-9;]*m", "", log)
res["log_levels"] = dict(collections.Counter(re.findall(r"\b(ERROR|WARN)\b", plain)))
msgs = collections.Counter(re.sub(r"\d+", "N", m.strip())[:160]
                           for m in re.findall(r"(?:ERROR|WARN)\s+\S+:\s*(.*)", plain))
res["log_top"] = msgs.most_common(6)
eng = os.path.join(D, "engine.err.log")
if os.path.exists(eng):
    el = open(eng, errors="replace").read()
    res["engine_errors"] = len(re.findall(r"<Error>", el))
    res["engine_error_top"] = collections.Counter(
        re.sub(r"\d+", "N", m)[:160] for m in re.findall(r"<Error> (.*)", el)).most_common(4)
print(json.dumps(res, indent=1))
