#!/usr/bin/env python3
"""03 · One service on two nodes behind an edge (see the .md; merges the old 03 and 04).

Part A, a native static site (Python) on lab-b and lab-c behind lab-a:
round robin, fail-over when one copy stops, balance methods.
Part B, a container (traefik/whoami) as one Compose stack per node behind
lab-a: both copies answer, fail-over, stack update, revisions and rollback,
container logs.
Part C (--node-down, Hyper-V host only): shut lab-c down; the edge keeps
serving from lab-b.
"""

from __future__ import annotations

import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from labkit.lab import Lab, parser, run  # noqa: E402

ENV = "s03"
SITE_A, DOMAIN_A = "s03-web", "s03.lab.local"
SITE_B, DOMAIN_B = "s03-whoami", "s03w.lab.local"


def whoami(tag: str) -> str:
    return f"""services:
  whoami:
    image: traefik/whoami:{tag}
    command: ["--port", "8090"]
    ports: ["0.0.0.0:8090:8090"]
    restart: unless-stopped
    # Stack ports bind to the node's internal address unless marked public
    # (loopback without a fabric); the edge on lab-a must reach them.
    x-forge: {{ public: [8090] }}
"""


def answers(lab: Lab, domain: str, n: int = 12) -> list[str]:
    out = []
    for _ in range(n):
        code, body = lab.http(domain)
        line = next((ln for ln in body.splitlines() if ln.startswith("Hostname") or "node" in ln), body.strip()[:40])
        out.append(f"{code} {line}")
    return out


def main() -> int:
    p = parser(__doc__)
    p.add_argument("--node-down", action="store_true", help="also shut lab-c down in Hyper-V (host, admin)")
    a = p.parse_args()
    lab = Lab(a, "03-load-balanced-two-nodes")
    lab.environment(ENV)
    edge = [lab.node_id("lab-a")]
    lab.check("03", "nginx on lab-a", lab.install_package("nginx", "stable", ["lab-a"]))

    if lab.wanted("03A"):
        lab.step("A · native site on lab-b and lab-c")
        lab.check("03A", "Python on both", lab.install_package("python", "system", ["lab-b", "lab-c"]))
        app = lab.create_app(ENV, {
            "name": "web", "runtime": "systemd", "listen_port": 8080, "restart": True, "env": [],
            "command": "python3 -m http.server 8080 --bind 0.0.0.0 --directory /srv/forge/apps/s03/web/site",
            "dependencies": [{"package": "python", "version": "system", "params": {}}],
        })
        r = lab.place_and_deploy("web", ENV, ["lab-b", "lab-c"])
        lab.check("03A", "deployed on both", r.ok, r.message)
        for n in ("lab-b", "lab-c"):
            z = lab.site_zip(f"site-{n}", f"<p>node {n}</p>")
            lab.forge("services", "files", "web", "--environment", ENV, "--node", n, "upload", str(z), "/site.zip", "--overwrite")
            lab.forge("services", "files", "web", "--environment", ENV, "--node", n, "extract", "/site.zip", "/site", "--replace")
        r = lab.site({"name": SITE_A, "domains": [DOMAIN_A], "upstreams": [], "balance": "round_robin", "tls": "off",
                      "edges": edge, "targets": [{"kind": "app", "service_id": app["id"], "port": 8080}]})
        lab.check("03A", "endpoint applied", r.ok, r.message)
        got = answers(lab, DOMAIN_A)
        lab.check("03A", "12 × 200 from both nodes", all(x.startswith("200") for x in got)
                  and any("lab-b" in x for x in got) and any("lab-c" in x for x in got), str(sorted(set(got))))
        lab.forge("services", "action", "web", "stop", "--node", "lab-b", "--environment", ENV, "--yes")
        time.sleep(2)
        got = answers(lab, DOMAIN_A)
        lab.check("03A", "fail-over: lab-b stopped, still 200 from lab-c", all(x.startswith("200") and "lab-c" in x for x in got), str(set(got)))
        lab.forge("services", "action", "web", "start", "--node", "lab-b", "--environment", ENV, "--yes")
        for balance in ("least_conn", "ip_hash"):
            r = lab.site({"name": SITE_A, "domains": [DOMAIN_A], "upstreams": [], "balance": balance, "tls": "off",
                          "edges": edge, "targets": [{"kind": "app", "service_id": app["id"], "port": 8080}]})
            got = answers(lab, DOMAIN_A, 8)
            same = len({x for x in got}) == 1
            ok = r.ok and all(x.startswith("200") for x in got) and (same if balance == "ip_hash" else True)
            lab.check("03A", f"balance {balance}", ok, str(set(got)))
        routing = lab.forge("clusters", "routing", "lab")
        lab.check("03A", "endpoint in the cluster routing view", routing.ok and SITE_A in str(routing.result), routing.message)

    if lab.wanted("03B"):
        lab.step("B · the same container on both nodes (one stack per node)")
        lab.check("03B", "Docker on both", lab.install_package("docker", "stable", ["lab-b", "lab-c"]))
        f = lab.work / "whoami.yml"
        f.write_text(whoami("v1.10.1"))
        for s, n in (("whoami-b", "lab-b"), ("whoami-c", "lab-c")):
            lab.drop_stack(s)
            lab.must("stacks", "create", s, "--environment", ENV, "--node", n, "-f", str(f))
            r = lab.forge("stacks", "deploy", s, "--wait", "--yes", timeout=1800)
            lab.check("03B", f"{s} deployed", r.ok, r.message)
        r = lab.site({"name": SITE_B, "domains": [DOMAIN_B], "upstreams": [], "balance": "least_conn", "tls": "off",
                      "edges": edge, "targets": [{"kind": "node", "server_id": lab.node_id("lab-b"), "port": 8090},
                                                 {"kind": "node", "server_id": lab.node_id("lab-c"), "port": 8090}]})
        lab.check("03B", "endpoint applied", r.ok, r.message)
        hosts = {x for x in answers(lab, DOMAIN_B, 16)}
        lab.check("03B", "both containers answer", len(hosts) == 2 and all(h.startswith("200") for h in hosts), str(hosts))
        lab.forge("stacks", "service", "whoami-b", "stop", "--node", "lab-b", "--services", "whoami", "--wait")
        time.sleep(2)
        hosts = {x for x in answers(lab, DOMAIN_B, 10)}
        lab.check("03B", "fail-over: one container stopped, no errors", len(hosts) == 1 and next(iter(hosts)).startswith("200"), str(hosts))
        lab.forge("stacks", "service", "whoami-b", "start", "--node", "lab-b", "--services", "whoami", "--wait")
        f.write_text(whoami("v1.11.0"))
        lab.forge("stacks", "update", "whoami-b", "-f", str(f))
        r = lab.forge("stacks", "deploy", "whoami-b", "--wait", "--yes", timeout=1800)
        revs = lab.forge("stacks", "revisions", "whoami-b")
        lab.check("03B", "update makes revision 2", r.ok and len(revs.result or []) >= 2, str(revs.result)[:120])
        rb = lab.forge("stacks", "rollback", "whoami-b", "--to", "1", "--wait", "--yes", timeout=1800)
        lab.check("03B", "rollback to revision 1", rb.ok, rb.message)
        logs = lab.forge("stacks", "logs", "whoami-b", "whoami", "--node", "lab-b", "--tail", "20")
        lab.check("03B", "container logs readable", logs.ok, logs.message)

    if a.node_down and lab.wanted("03C"):
        lab.step("C · lab-c shut down")
        subprocess.run(["powershell", "-NoProfile", "-Command", "Stop-VM LAB-C -TurnOff -Force"], check=False)
        time.sleep(5)
        got = answers(lab, DOMAIN_B, 10)
        lab.check("03C", "edge keeps serving from lab-b", all(x.startswith("200") for x in got), str(set(got)))
        end = time.time() + 180
        offline = False
        while time.time() < end and not offline:
            offline = lab.nodes().get("lab-c", {}).get("status") == "offline"
            time.sleep(5)
        lab.check("03C", "Forge shows lab-c offline", offline)
        subprocess.run(["powershell", "-NoProfile", "-Command", "Start-VM LAB-C"], check=False)

    if not a.keep:
        lab.step("clean up")
        for s in (SITE_A, SITE_B):
            lab.drop_endpoint(s)
        for s in ("whoami-b", "whoami-c"):
            lab.drop_stack(s)
        lab.drop_service("web", ENV)
    return lab.finish()


if __name__ == "__main__":
    run(main)
