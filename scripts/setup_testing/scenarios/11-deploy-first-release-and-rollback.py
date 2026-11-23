#!/usr/bin/env python3
"""11 · First release, in-place updates, rollback and the lock (see the .md).

D01 recreate creates the service; the image tag is pinned to a digest.
D02 stop-first on one node is refused unless downtime is accepted.
D08 rollback and redeploy.
D15 a second deployment while one runs is refused (the lock).
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from labkit.lab import Lab, image_doc, parser, run  # noqa: E402

ENV, NAME = "dev", "s11-web"
V1, V2 = "traefik/whoami:v1.10.1", "traefik/whoami:v1.11.0"
HEALTH = {
    "startupTimeout": "3m",
    "stabilizationPeriod": "5s",
    "checks": {"readiness": [{"name": "http", "probe": {"type": "http", "port": 8080, "path": "/health"}}]},
}


def doc(image: str, strategy: dict, description: str = "") -> dict:
    return image_doc(NAME, ENV, image, ports=["8080:80"], strategy=strategy, health=HEALTH,
                     placement={"nodes": ["lab-c"]}, description=description)


def main() -> int:
    a = parser(__doc__).parse_args()
    lab = Lab(a, "11-deploy-first-release-and-rollback")
    node = a.docker_node

    if lab.wanted("D01"):
        lab.step("D01 first release with recreate")
        r = lab.apply(doc(V1, {"type": "recreate"}, "first release"))
        lab.check("D01", "apply --wait exits 0", r.ok, r.message)
        rels = lab.must("deploy", "releases", NAME, "-n", ENV) or []
        lab.check("D01", "release 1 exists", bool(rels) and rels[-1]["number"] == 1)
        digest = (rels[0]["artifact"] or {}).get("digest") if rels else None
        lab.check("D01", "the tag was pinned to a digest", bool(digest and digest.startswith("sha256:")), str(digest))
        out = lab.node_sh(node, f"docker ps --format '{{{{.Names}}}}' | grep -x forge-app-{ENV}-{NAME}")
        lab.check("D01", "the single instance runs on the node", out.ok, out.message)
        r = lab.node_sh(node, "curl -fsS http://127.0.0.1:8080/health")
        lab.check("D01", "it answers on the service port", r.ok, r.message)

    if lab.wanted("D02"):
        lab.step("D02 stop-first on one node")
        r = lab.plan(doc(V2, {"type": "rolling", "replacement": "stop-first"}))
        lab.check("D02", "refused without allowDowntime", not r.ok and "allowDowntime" in r.message, r.message[:160])
        r = lab.apply(doc(V2, {"type": "rolling", "replacement": "stop-first", "allowDowntime": True}, "v1.11"))
        lab.check("D02", "accepted with allowDowntime", r.ok, r.message)
        hist = lab.must("deploy", "history", NAME, "-n", ENV) or []
        lab.check("D02", "release 2 succeeded", bool(hist) and hist[0]["release_number"] == 2 and hist[0]["state"] == "succeeded")

    if lab.wanted("D08"):
        lab.step("D08 rollback and redeploy")
        r = lab.forge("deploy", "rollback", NAME, "-n", ENV, "--wait", "--timeout", str(a.timeout), timeout=a.timeout * 60 + 60)
        lab.check("D08", "rollback --wait exits 0", r.ok, r.message)
        cur = lab.slots(NAME, ENV)
        lab.check("D08", "release 1 serves again", cur.get("current_release") == 1, str(cur.get("current_release")))
        hist = lab.must("deploy", "history", NAME, "-n", ENV) or []
        lab.check("D08", "history records a rollback", bool(hist) and hist[0]["kind"] == "rollback")
        r = lab.forge("deploy", "redeploy", NAME, "2", "-n", ENV, "--wait", "--timeout", str(a.timeout), timeout=a.timeout * 60 + 60)
        lab.check("D08", "redeploy of release 2 exits 0", r.ok, r.message)
        lab.check("D08", "release 2 serves", lab.slots(NAME, ENV).get("current_release") == 2)

    if lab.wanted("D15"):
        lab.step("D15 one deployment at a time")
        slow = dict(HEALTH, stabilizationPeriod="90s")
        first = image_doc(NAME, ENV, V1, ports=["8080:80"], strategy={"type": "recreate"}, health=slow,
                          placement={"nodes": ["lab-c"]})
        r1 = lab.apply(first, wait=False)
        dep = (r1.result or {}).get("id") if r1.ok else None
        lab.check("D15", "the first deployment starts", bool(dep), r1.message)
        r2 = lab.apply(first, wait=False)
        lab.check("D15", "a second one is refused (exit 7, conflict)", r2.code == 7 and "already" in r2.message, r2.message[:160])
        if dep:
            lab.forge("deploy", "abort", dep)
            d = lab.wait_state(dep, {"aborted", "failed", "succeeded", "rolled_back"}, timeout=300)
            lab.check("D15", "abort ends it and frees the service", d["state"] in {"aborted", "succeeded"}, d["state"])

    if not a.keep:
        lab.step("clean up")
        lab.drop_service(NAME, ENV)
    return lab.finish()


if __name__ == "__main__":
    run(main)
