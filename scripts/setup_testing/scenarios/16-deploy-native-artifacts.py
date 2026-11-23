#!/usr/bin/env python3
"""16 · Native services from fetched code (see the .md).

D09 a .tar.gz release asset: fetched into <home>/releases/<n>, checked by
SHA-256, run as the service's own account on $PORT.
D10 a .zip with a top folder (stripComponents: 1), start-first on two nodes
behind an endpoint: two copies side by side per node, no failed request.
D11 a pinned Git commit.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from labkit.lab import Lab, Traffic, parser, run  # noqa: E402

def ready(port: int) -> dict:
    return {"readiness": [{"name": "ready", "probe": {"type": "http", "port": port, "path": "/ready"}}]}


def native(name: str, env: str, artifact: dict, strategy: dict, nodes: list[str], runtime: str = "systemd",
           port: int = 8090) -> dict:
    template = {"command": "python3 app.py", "listenPort": port} if runtime == "systemd" else {"command": "sh run.sh"}
    spec = {
        "runtime": runtime, "artifact": artifact, "template": template,
        "placement": {"nodes": nodes}, "strategy": strategy, "traffic": {"drainTimeout": "5s"},
        "health": {"startupTimeout": "2m", "stabilizationPeriod": "5s",
                   "checks": ready(port) if runtime == "systemd" else {}},
    }
    if port != 8090:
        # In place the service gets no $PORT from a slot: set it (D09 holds 8090 on lab-b).
        spec["env"] = {"PORT": str(port)}
    return {"apiVersion": "forge/v1", "kind": "Deployment", "metadata": {"name": name, "environment": env}, "spec": spec}


def main() -> int:
    a = parser(__doc__).parse_args()
    lab = Lab(a, "16-deploy-native-artifacts")
    mock, fx = lab.state["mock"], lab.state["fixtures"]
    edge_node = lab.state.get("edge_node", "lab-a")
    asset = lambda v, kind: {"url": mock + fx["hello-py"][v][kind]["path"], "sha256": fx["hello-py"][v][kind]["sha256"]}  # noqa: E731

    if lab.wanted("D09"):
        lab.step("D09 a .tar.gz release asset")
        r = lab.apply(native("s16-py", "dev", {"archive": asset("v1", "tar")}, {"type": "recreate"}, ["lab-b"]))
        lab.check("D09", "release 1 went live", r.ok, r.message)
        out = lab.node_sh("lab-b", "curl -fsS http://127.0.0.1:8090/")
        lab.check("D09", "it answers hello-py v1 on its port", out.ok and "hello-py v1" in str(out.result), out.message or str(out.result))
        d = lab.node_sh("lab-b", "stat -c '%U %n' /srv/forge/apps/dev/s16-py/releases/1/app.py && cat /srv/forge/apps/dev/s16-py/releases/1/.forge-release")
        lab.check("D09", "the code is in releases/1, owned by the service's account", d.ok and "dev-s16-py" in str(d.result), str(d.result or d.message))
        ps = lab.node_sh("lab-b", "ps -o user= -C python3 | sort -u")
        lab.check("D09", "it runs as its own account, not root", ps.ok and "root" not in str(ps.result), str(ps.result))
        # Free port 8090 on lab-b for the next checks.
        lab.drop_service("s16-py", "dev")

    if lab.wanted("D10"):
        lab.step("D10 a .zip (stripComponents 1), start-first on two nodes behind an endpoint")
        zip_ = lambda v: dict(asset(v, "zip"), stripComponents=1)  # noqa: E731
        r = lab.apply(native("s16-py", "staging", {"archive": zip_("v1")}, {"type": "recreate"}, ["lab-b", "lab-c"]))
        lab.check("D10", "release 1 on both nodes", r.ok, r.message)
        app = lab.service("s16-py", "staging")
        lab.endpoint("s16-py", "s16.lab.local", app["id"], 8090, [lab.node_id(edge_node)])  # type: ignore[index]
        t = Traffic(a.edge, "s16.lab.local").start()
        r = lab.apply(native("s16-py", "staging", {"archive": zip_("v2")}, {"type": "rolling", "replacement": "start-first"}, ["lab-b", "lab-c"]))
        t.stop()
        lab.check("D10", "release 2 went live", r.ok, r.message)
        lab.check("D10", "no failed request", t.failures == 0 and t.total > 20, f"{t.total} requests, {t.statuses}")
        lab.check("D10", "every answer is v2 now", t.sample(20) == {"hello-py v2": 20}, str(t.sample(5)))
        slots = lab.slots("s16-py", "staging")["slots"]
        lab.check("D10", "each node serves from slot a on its own port", sorted(s["slot"] for s in slots) == ["a", "a"], str(slots))
        units = lab.node_sh("lab-c", "systemctl is-active forge-app-staging-s16-py--a.service")
        lab.check("D10", "the slot has its own unit", units.ok, units.message)

    if lab.wanted("D11"):
        lab.step("D11 a pinned Git commit")
        repo = mock + fx["git"]["repo_path"]
        r = lab.apply(native("s16-git", "dev", {"git": {"repo": repo, "commit": fx["git"]["v2"]}}, {"type": "recreate"}, ["lab-b"], port=8091))
        lab.check("D11", "the commit went live", r.ok, r.message)
        out = lab.node_sh("lab-b", "cat /srv/forge/apps/dev/s16-git/releases/1/VERSION; cat /srv/forge/apps/dev/s16-git/releases/1/.forge-release")
        lab.check("D11", "exactly that commit was checked out", out.ok and fx["git"]["v2"] in str(out.result), str(out.result or out.message))

    if not a.keep:
        lab.step("clean up")
        lab.drop_endpoint("s16-py")
        for name, env in (("s16-py", "dev"), ("s16-py", "staging"), ("s16-git", "dev")):
            lab.drop_service(name, env)
    return lab.finish()


if __name__ == "__main__":
    run(main)
