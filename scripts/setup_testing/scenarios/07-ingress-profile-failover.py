#!/usr/bin/env python3
"""07 · Cluster ingress profile with two edges (see the .md).

lab-a and lab-b serve a cluster-linked endpoint (active-active); one edge's
nginx stops and the other keeps serving; a tuning change applies to both.
"""

from __future__ import annotations

import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from labkit.lab import Lab, parser, run  # noqa: E402

ENV, SITE, DOMAIN = "s07", "s07-web", "s07.lab.local"
EDGES = {"lab-a": "10.50.0.10", "lab-b": "10.50.0.11"}


def main() -> int:
    p = parser(__doc__)
    p.add_argument("--edge-b", default="10.50.0.11", help="lab-b as this machine reaches it (WSL: 127.0.0.1:18082)")
    a = p.parse_args()
    lab = Lab(a, "07-ingress-profile-failover")
    lab.environment(ENV)
    reach = {"lab-a": a.edge, "lab-b": a.edge_b}

    lab.step("nginx on both edges")
    lab.check("07", "nginx on lab-a and lab-b", lab.install_package("nginx", "stable", ["lab-a", "lab-b"]))

    lab.step("ingress profile: active-active on lab-a and lab-b")
    edges = [f"{n}={ip}" for n, ip in EDGES.items()]
    r = lab.forge("clusters", "ingress", "set", "lab", *sum((["--edge", e] for e in edges), []), "--mode", "active_active", "--yes")
    lab.check("07", "profile saved", r.ok, r.message)
    r = lab.forge("clusters", "ingress", "apply", "lab", "--yes", timeout=900)
    lab.check("07", "profile applied", r.ok, r.message)

    lab.step("app and cluster-linked endpoint")
    lab.check("07", "Python on lab-c", lab.install_package("python", "system", ["lab-c"]))
    app = lab.create_app(ENV, {
        "name": "web", "runtime": "systemd", "listen_port": 8080, "restart": True, "env": [],
        "command": "python3 -m http.server 8080 --bind 0.0.0.0 --directory /srv/forge/apps/s07/web",
        "dependencies": [{"package": "python", "version": "system", "params": {}}],
    })
    r = lab.place_and_deploy("web", ENV, ["lab-c"])
    lab.check("07", "web deployed on lab-c", r.ok, r.message)
    r = lab.site({"name": SITE, "domains": [DOMAIN], "upstreams": [], "tls": "off", "balance": "round_robin",
                  "cluster_id": lab.cluster_id("lab"), "targets": [{"kind": "app", "service_id": app["id"], "port": 8080}]})
    lab.check("07", "endpoint applied through the profile", r.ok, r.message)
    for n in EDGES:
        code, _ = lab.http(DOMAIN, edge=reach[n])
        lab.check("07", f"{n} serves the endpoint", code == 200, str(code))
    routing = lab.forge("clusters", "routing", "lab")
    lab.check("07", "routing view lists the endpoint", SITE in json.dumps(routing.result), routing.message)

    lab.step("edge fail-over")
    lab.node_sh("lab-b", "systemctl stop nginx")
    lab.check("07", "lab-a still serves while lab-b is down", lab.http(DOMAIN, edge=reach["lab-a"])[0] == 200)
    lab.node_sh("lab-b", "systemctl start nginx")
    time.sleep(3)
    lab.check("07", "lab-b serves again", lab.http(DOMAIN, edge=reach["lab-b"])[0] == 200)

    lab.step("tuning change on both edges")
    prof = lab.must("clusters", "ingress", "get", "lab")
    profile = (prof or {}).get("profile") or prof  # type: ignore[union-attr]
    ingress = profile.get("ingress", {}) if isinstance(profile, dict) else {}
    ingress.setdefault("nginx", {})["client_max_body_mb"] = 64
    f = lab.work / "profile.json"
    f.write_text(json.dumps({"ingress": ingress}))
    r = lab.forge("clusters", "ingress", "set", "lab", "--file", str(f), "--yes")
    r2 = lab.forge("clusters", "ingress", "apply", "lab", "--yes", timeout=900)
    lab.check("07", "changed profile saved and applied", r.ok and r2.ok, r.message or r2.message)
    for n in EDGES:
        t = lab.node_sh(n, "nginx -t 2>&1")
        lab.check("07", f"nginx -t passes on {n}", t.ok, t.message)
    lab.check("07", "endpoint still works", lab.http(DOMAIN)[0] == 200)

    if not a.keep:
        lab.step("clean up")
        lab.drop_endpoint(SITE)
        lab.drop_service("web", ENV)
        lab.forge("clusters", "ingress", "remove", "lab", "--yes", timeout=900)
    return lab.finish()


if __name__ == "__main__":
    run(main)
