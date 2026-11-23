"""Operators using the console: each user polls the logs search and
histogram, the nodes list, one node's history and its container metrics
every 2 s.

Usage: python3 -I readers.py <api_base> <password_file> <users> <seconds> <out.jsonl>
Writes one JSON line per request: {"t", "ep", "status", "ms"}.
"""
import json, os, random, sys, threading, time, urllib.parse

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from forge_api import Api  # noqa: E402

BASE, PWFILE, USERS, SECS, OUT = sys.argv[1], sys.argv[2], int(sys.argv[3]), float(sys.argv[4]), sys.argv[5]
PW = open(PWFILE).read().strip()
lock, out = threading.Lock(), open(OUT, "a", buffering=1)
WORDS = ["timeout", "order", "cache miss", "logged in", "ERROR"]


def rec(ep, s, dt):
    with lock:
        out.write(json.dumps({"t": round(time.time(), 2), "ep": ep, "status": s, "ms": round(dt * 1000, 1)}) + "\n")


def user(n, end):
    api = Api(BASE, "admin", PW)
    nodes = []
    while time.time() < end:
        t0 = time.time()
        now = int(t0 * 1000)
        win = {"from": now - 15 * 60_000, "to": now}
        s, dt, v = api.get("/api/v1/servers")
        rec("servers", s, dt)
        if isinstance(v, list) and v:
            nodes = [x["id"] for x in v if x.get("id")]
        q = dict(win, q=random.choice(WORDS), limit=100)
        s, dt, _ = api.get("/api/v1/logs?" + urllib.parse.urlencode(q))
        rec("logs_search", s, dt)
        s, dt, _ = api.get("/api/v1/logs/histogram?" + urllib.parse.urlencode(q))
        rec("logs_histogram", s, dt)
        if nodes:
            node = random.choice(nodes)
            hw = {"from": now - 3_600_000, "to": now}
            s, dt, _ = api.get(f"/api/v1/servers/{node}/history?" + urllib.parse.urlencode(hw))
            rec("node_history", s, dt)
            s, dt, _ = api.get(f"/api/v1/servers/{node}/container-metrics?" + urllib.parse.urlencode(hw))
            rec("container_metrics", s, dt)
        time.sleep(max(0.0, 2.0 - (time.time() - t0)))


end = time.time() + SECS
ts = [threading.Thread(target=user, args=(i, end), daemon=True) for i in range(USERS)]
for t in ts:
    t.start()
    time.sleep(0.4)
for t in ts:
    t.join()
