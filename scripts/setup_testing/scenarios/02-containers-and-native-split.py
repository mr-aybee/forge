#!/usr/bin/env python3
"""02 · Containers on one node, native apps on the other (see the .md).

Seq as a Compose stack on lab-b (Docker); a Python API on lab-c that logs to
Seq; optionally (--with-dotnet) a .NET service on lab-c. Seq runs without
authentication here (lab only), so the script can read its events back.
"""

from __future__ import annotations

import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from labkit.lab import Lab, parser, run  # noqa: E402

ENV = "s02"
SEQ = """services:
  seq:
    image: datalust/seq:2024.3
    environment:
      ACCEPT_EULA: "Y"
      SEQ_FIRSTRUN_NOAUTHENTICATION: "true"
    ports:
      - "127.0.0.1:5380:80"
      - "10.50.0.11:5341:5341"
    volumes: ["seqdata:/data"]
    restart: unless-stopped
volumes: { seqdata: {} }
"""
API = '''import json, os, datetime, urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer
SEQ = os.environ["SEQ_URL"]
def log(msg):
    body = json.dumps({"@t": datetime.datetime.utcnow().isoformat() + "Z", "@mt": msg}).encode()
    req = urllib.request.Request(SEQ + "/ingest/clef", body, {"Content-Type": "application/vnd.serilog.clef"})
    try:
        urllib.request.urlopen(req, timeout=2)
    except Exception as e:
        print("seq:", e, flush=True)
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        log("GET " + self.path)
        self.send_response(200); self.end_headers(); self.wfile.write(b"api ok")
HTTPServer(("127.0.0.1", 8001), H).serve_forever()
'''


def main() -> int:
    p = parser(__doc__)
    p.add_argument("--with-dotnet", action="store_true", help="also the .NET service (installs the .NET 8 SDK on lab-c)")
    a = p.parse_args()
    lab = Lab(a, "02-containers-and-native-split")
    lab.environment(ENV)

    lab.step("runtimes")
    lab.check("02", "Docker on lab-b", lab.install_package("docker", "stable", ["lab-b"]))
    lab.check("02", "Python on lab-c", lab.install_package("python", "system", ["lab-c"]))

    lab.step("Seq as a Compose stack on lab-b")
    f = lab.work / "seq.yml"
    f.write_text(SEQ)
    lab.drop_stack("seq")
    lab.must("stacks", "create", "seq", "--environment", ENV, "--node", "lab-b", "-f", str(f))
    r = lab.forge("stacks", "deploy", "seq", "--wait", "--yes", timeout=1800)
    lab.check("02", "Seq deployed", r.ok, r.message)
    ok = False
    for _ in range(60):
        if lab.node_sh("lab-b", "curl -fsS -o /dev/null http://127.0.0.1:5380/api").ok:
            ok = True
            break
        time.sleep(3)
    lab.check("02", "Seq answers on lab-b", ok)

    lab.step("Python API on lab-c logging to Seq")
    lab.create_app(ENV, {
        "name": "api", "runtime": "systemd", "listen_port": 8001, "restart": True,
        "command": "python3 /srv/forge/apps/s02/api/app.py",
        "env": [{"key": "SEQ_URL", "value": "http://10.50.0.11:5341"}],
        "dependencies": [{"package": "python", "version": "system", "params": {}}],
    })
    r = lab.place_and_deploy("api", ENV, ["lab-c"])
    lab.check("02", "api deployed", r.ok, r.message)
    app_py = lab.work / "app.py"
    app_py.write_text(API)
    up = lab.forge("services", "files", "api", "--environment", ENV, "upload", str(app_py), "/app.py", "--overwrite")
    lab.check("02", "code uploaded", up.ok, up.message)
    lab.forge("services", "action", "api", "restart", "--node", "lab-c", "--environment", ENV, "--yes")
    time.sleep(3)
    for i in range(5):
        lab.node_sh("lab-c", f"curl -s http://127.0.0.1:8001/ping{i}")
    r = lab.node_sh("lab-c", "curl -s http://127.0.0.1:8001/")
    lab.check("02", "api answers", "api ok" in str(r.result), str(r.result))
    seen = 0
    for _ in range(15):
        ev = lab.node_sh("lab-b", "curl -s 'http://127.0.0.1:5380/api/events?count=50'")
        try:
            seen = sum(1 for e in json.loads(str(ev.result)) if "ping" in json.dumps(e))
        except (ValueError, TypeError):
            seen = 0
        if seen >= 5:
            break
        time.sleep(2)
    lab.check("02", "Seq has the 5 events from lab-c", seen >= 5, f"{seen} events")

    lab.step("restart Seq: events survive (volume)")
    lab.forge("stacks", "service", "seq", "restart", "--node", "lab-b", "--services", "seq", "--wait")
    time.sleep(15)
    ev = lab.node_sh("lab-b", "curl -s 'http://127.0.0.1:5380/api/events?count=50'")
    lab.check("02", "events kept after restart", "ping" in str(ev.result))
    logs = lab.forge("stacks", "logs", "seq", "seq", "--node", "lab-b", "--tail", "20")
    lab.check("02", "stack logs readable", logs.ok, logs.message)
    lst = lab.forge("services", "list", "--environment", ENV)
    names = sorted(x.get("name") for x in (lst.result or []))
    lab.check("02", "apps list shows api and the Seq stack service", "api" in names and any(n.startswith("seq") for n in names), str(names))

    if a.with_dotnet:
        lab.step(".NET service on lab-c")
        lab.check("02", ".NET SDK on lab-c", lab.install_package("dotnet", "8.0", ["lab-c"], {"flavor": "sdk"}))
        lab.create_app(ENV, {
            "name": "dot", "runtime": "systemd", "listen_port": 8002, "restart": True, "env": [],
            "command": "dotnet /srv/forge/apps/s02/dot/out/dot.dll --urls http://127.0.0.1:8002",
            "dependencies": [{"package": "dotnet", "version": "8.0", "params": {"flavor": "sdk"}}],
        })
        lab.place_and_deploy("dot", ENV, ["lab-c"])
        b = lab.node_sh("lab-c", "cd /tmp && rm -rf dotsrc && . /etc/forge/pkgs/dotnet.env 2>/dev/null; dotnet new web -o dotsrc --force >/dev/null "
                        "&& dotnet publish dotsrc -c Release -o /srv/forge/apps/s02/dot/out >/dev/null && chown -R s02-dot: /srv/forge/apps/s02/dot/out",
                        timeout=1800)
        lab.check("02", ".NET app published on the node", b.ok, b.message)
        lab.forge("services", "action", "dot", "restart", "--node", "lab-c", "--environment", ENV, "--yes")
        time.sleep(8)
        r = lab.node_sh("lab-c", "curl -s http://127.0.0.1:8002/")
        lab.check("02", ".NET answers Hello World", "Hello World" in str(r.result), str(r.result))

    if not a.keep:
        lab.step("clean up")
        lab.drop_stack("seq")
        for n in ("api", "dot"):
            lab.drop_service(n, ENV)
    return lab.finish()


if __name__ == "__main__":
    run(main)
