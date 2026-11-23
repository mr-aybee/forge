#!/usr/bin/env python3
"""06 · Two clusters, teams and environments (see the .md).

Clusters `web` (lab-b) and `data` (lab-c), environments in each, two users
with scoped roles, and what each may see and do.
"""

from __future__ import annotations

import secrets
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from labkit.lab import Lab, parser, run  # noqa: E402
from labkit.session import Session  # noqa: E402


def main() -> int:
    a = parser(__doc__).parse_args()
    lab = Lab(a, "06-multi-cluster-environments")
    nodes = lab.nodes()

    lab.step("clusters and moving nodes")
    cl = {c["name"]: c["id"] for c in lab.must("clusters", "list") or []}
    for name, abbr in (("web", "WEB"), ("data", "DAT")):
        if name not in cl:
            r = lab.api("POST", "/api/v1/clusters", {"name": name, "description": f"{name} team", "abbr": abbr})
            lab.check("06", f"cluster {name} created", r.ok, r.message)
            cl[name] = (r.result or {}).get("id")
    for node, cluster in (("lab-b", "web"), ("lab-c", "data")):
        r = lab.api("PATCH", f"/api/v1/servers/{nodes[node]['id']}", {"cluster_id": cl[cluster]})
        lab.check("06", f"{node} moved to {cluster}", r.ok, r.message)

    lab.step("environments")
    ns = {}
    for name, cluster in (("web-prod", "web"), ("web-dev", "web"), ("data-prod", "data")):
        lab.environment("lab-anchor")  # makes sure the `lab` project exists
        lab.forge("environments", "create", name, "--cluster", cluster, "--project", "lab")
        ns[name] = next((n["id"] for n in lab.must("environments", "list", "--all") or [] if n["name"] == name), None)
        lab.check("06", f"environment {name}", bool(ns[name]))

    lab.step("users with scoped roles")
    pw = {u: secrets.token_urlsafe(18) for u in ("alice", "bob")}
    for u in pw:
        lab.api("POST", "/api/v1/users", {"username": u, "email": f"{u}@lab.local", "display_name": u, "password": pw[u]})
    r1 = lab.api("POST", "/api/v1/role-bindings", {"email": "alice", "role": "operator", "scope": f"cluster:{cl['web']}"})
    r2 = lab.api("POST", "/api/v1/role-bindings", {"email": "bob", "role": "developer", "scope": f"environment:{ns['data-prod']}"})
    lab.check("06", "alice: operator on cluster web", r1.ok, r1.message)
    lab.check("06", "bob: developer on data-prod", r2.ok, r2.message)

    lab.step("isolation")
    alice = Session(a.server, "alice", pw["alice"])
    st, servers = alice.call("GET", "/api/v1/servers")
    seen = sorted(s["hostname"] for s in servers or []) if st == 200 else []
    lab.check("06", "alice sees lab-b only", seen == ["lab-b"], str(seen))
    st, _ = alice.call("GET", f"/api/v1/servers/{nodes['lab-c']['id']}")
    lab.check("06", "alice cannot see lab-c (403/404)", st in (403, 404), str(st))
    bob = Session(a.server, "bob", pw["bob"])
    st, napps = bob.call("GET", f"/api/v1/services?environment_id={ns['web-prod']}")
    lab.check("06", "bob sees no web-prod services", st in (200, 403) and not (napps or []) if st == 200 else True, str(st))
    st, _ = bob.call("POST", "/api/v1/packages/install", {"package": "nginx", "version": "stable", "params": {}, "server_ids": [nodes["lab-c"]["id"]]})
    lab.check("06", "bob (developer) cannot install packages (403)", st == 403, str(st))
    audit = lab.forge("audit", "list", "--limit", "50")
    denials = [e for e in (audit.result or []) if e.get("outcome") in ("denied", "failure")]
    lab.check("06", "denials appear in the audit log", bool(denials), f"{len(denials)} denials")

    if not a.keep:
        lab.step("clean up: nodes back to lab")
        for node in ("lab-b", "lab-c"):
            lab.api("PATCH", f"/api/v1/servers/{nodes[node]['id']}", {"cluster_id": lab.cluster_id("lab")})
    return lab.finish()


if __name__ == "__main__":
    run(main)
