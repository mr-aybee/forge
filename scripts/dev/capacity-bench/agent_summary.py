"""Real-agent figures of one phase: sent (docker logs), acked (container
lines in archived spool files whose id the worker's seen.log lists),
stored and distinct (chDB rows of the agent's node and the phase's
containers), plus a 15 s time series joined with sample.jsonl.

The archive misses files acked within 0.25 s of sealing, so the acked
count is a lower bound; a file left in the spool or refused is the check.

Usage: python3 -I agent_summary.py <bench_dir> <phase> <server_id>
"""
import base64, json, os, subprocess, sys, urllib.request

B, PH, NODE = sys.argv[1:4]
O = os.path.join(B, "results", PH)


def q(sql):
    pw = open(os.path.join(B, "data", "analytics", "secret")).read().strip()
    r = urllib.request.Request("http://127.0.0.1:18124/", sql.encode(), {
        "Authorization": "Basic " + base64.b64encode(f"forge:{pw}".encode()).decode()})
    return urllib.request.urlopen(r, timeout=60).read().decode().strip()


def jl(name):
    return [json.loads(l) for l in open(os.path.join(O, name)) if l.strip()]


seen = set()
for root, _, files in os.walk(os.path.join(B, "data")):
    if "seen.log" in files:
        for l in open(os.path.join(root, "seen.log")):
            p = l.split()
            if len(p) == 2 and p[0] == NODE:
                seen.add(p[1])
arch = os.path.join(O, "archive")
acked = files_acked = archived = 0
for n in sorted(os.listdir(arch)):
    archived += 1
    head = subprocess.run(f"zstd -dc '{os.path.join(arch, n)}' | head -1", shell=True, capture_output=True, text=True).stdout
    c = json.loads(head)["counts"]["container_log"]
    fid = n.split(".")[0].rsplit("-", 1)[1]
    if fid in seen:
        acked += c
        files_acked += 1
cnt = q(f"SELECT count(), uniqExact(ts, message, container_id, stream) FROM forge_op_analytics.app_logs "
        f"WHERE server_id = '{NODE}' AND source = 'container' AND container_name LIKE 'bench-{PH}-%' FORMAT TSV").split()
res = {"sent": int(open(os.path.join(O, "agent_sent")).read()), "acked_in_archived_files": acked, "node_files_in_seen_log": len(seen), "stored": int(cnt[0]),
       "distinct": int(cnt[1]), "files_archived": archived, "files_acked": files_acked}
a, s = jl("agent.jsonl"), jl("sample.jsonl")
t0 = min(a[0]["t"], s[0]["t"])
ready = float(open(os.path.join(O, "ready_t")).read())
SECS = {"main": 240, "stop": 180}[PH]
rows, prev = [], None
for r in a:
    if prev and r["t"] - prev["t"] > 0:
        r["up_kbs"] = round((r["sent_to_56132"] - prev["sent_to_56132"]) / (r["t"] - prev["t"]) / 1024, 1)
    prev = r
for r in a:
    t = r["t"] - t0
    srv = min(s, key=lambda x: abs(x["t"] - r["t"]))
    ph = "idle" if r["t"] < ready else "load" if r["t"] < ready + SECS else "after"
    rows.append({"t": round(t), "phase": ph, "a_cpu": r.get("agent_cpu_core_pct"), "a_rss": r.get("agent_VmRSS_mb"),
                 "a_anon": r.get("agent_RssAnon_mb"), "h_cpu": r.get("helper_cpu_core_pct"), "h_rss": r.get("helper_VmRSS_mb"),
                 "spool_files": r["spool_files"], "spool_kb": round(r["spool_bytes"] / 1024, 1), "up_kbs": r.get("up_kbs"),
                 "s_cpu": srv.get("server_cpu_core_pct"), "s_rss": srv.get("server_rss_mb"), "w_cpu": srv.get("worker_cpu_core_pct"),
                 "w_rss": srv.get("worker_rss_mb"), "e_cpu": srv.get("engine_cpu_core_pct"), "e_anon": srv.get("engine_anon_mb"),
                 "rows": srv.get("rows"), "sim_spool": srv.get("spool_files")})
res["upload_total_kb"] = round((a[-1]["sent_to_56132"] - a[0]["sent_to_56132"]) / 1024, 1)
res["agent_fds_max"] = max(r.get("agent_fds", 0) for r in a)
for k in ("a_cpu", "a_rss", "h_cpu", "h_rss", "up_kbs", "spool_files", "spool_kb"):
    for ph in ("idle", "load", "after"):
        v = [r[k] for r in rows if r["phase"] == ph and r[k] is not None]
        if v:
            res[f"{k}_{ph}"] = {"avg": round(sum(v) / len(v), 1), "max": max(v), "last": v[-1]}
json.dump(res, sys.stdout, indent=1)
with open(os.path.join(O, "series.json"), "w") as f:
    json.dump(rows, f)
