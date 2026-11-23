#!/usr/bin/env python3
"""12 · Zero-downtime rolling updates behind an endpoint (see the .md).

D03 start-first: the new release starts next to the old one (slot a, then
b), is gated, takes the traffic, and the old copy drains and stops — while
requests run through the edge, none of which may fail.
"""

from __future__ import annotations

import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from labkit.lab import Lab, Traffic, image_doc, parser, run  # noqa: E402

ENV, NAME, SITE, DOMAIN = "staging", "s12-web", "s12-web", "s12.lab.local"
V1, V2 = "traefik/whoami:v1.10.1", "traefik/whoami:v1.11.0"
HEALTH = {
    "startupTimeout": "3m",
    "stabilizationPeriod": "10s",
    "checks": {"readiness": [{"name": "http", "probe": {"type": "http", "port": 8080, "path": "/health"}}]},
}


def doc(image: str, strategy: dict) -> dict:
    return image_doc(NAME, ENV, image, ports=["8080:80"], strategy=strategy, health=HEALTH,
                     placement={"nodes": ["lab-c"]}, traffic={"drainTimeout": "10s"})


def serving(lab: Lab) -> list[tuple[str, int]]:
    return sorted((s["slot"], s["weight"]) for s in lab.slots(NAME, ENV)["slots"])


def main() -> int:
    a = parser(__doc__).parse_args()
    lab = Lab(a, "12-deploy-zero-downtime-rolling")
    edge_node = lab.state.get("edge_node", "lab-a")

    lab.step("first release in place, then the endpoint")
    r = lab.apply(doc(V1, {"type": "recreate"}))
    lab.check("D03", "release 1 (recreate)", r.ok, r.message)
    app = lab.service(NAME, ENV)
    lab.endpoint(SITE, DOMAIN, app["id"], 8080, [lab.node_id(edge_node)])  # type: ignore[index]
    t = Traffic(a.edge, DOMAIN, "/health")
    lab.check("D03", "the endpoint serves release 1", t.sample(5).get("", 0) >= 0 and t.failures == 0, str(t.statuses))

    if lab.wanted("D03"):
        for n, image, slot in ((2, V2, "a"), (3, V1, "b")):
            lab.step(f"D03 start-first to release {n} under load")
            t = Traffic(a.edge, DOMAIN, "/health").start()
            time.sleep(2)
            r = lab.apply(doc(image, {"type": "rolling", "replacement": "start-first"}))
            time.sleep(2)
            t.stop()
            lab.check("D03", f"release {n} went live", r.ok, r.message)
            lab.check("D03", f"no failed request during release {n}", t.failures == 0 and t.total > 20,
                      f"{t.total} requests, statuses {t.statuses}")
            lab.check("D03", f"slot {slot} alone serves", serving(lab) == [(slot, 100)], str(serving(lab)))
            ps = lab.node_sh(a.docker_node, "docker ps --format '{{.Names}}' | grep forge-app-staging-s12-web")
            names = (str(ps.result or "") if ps.ok else "").split()
            lab.check("D03", "the old copy was stopped", names == [f"forge-app-{ENV}-{NAME}--{slot}"], str(names))
            conf = lab.node_sh(edge_node, f"cat /etc/nginx/forge/sites/{SITE}.conf")
            lab.check("D03", "the edge points at the slot's port, no weights left", conf.ok and "weight=" not in str(conf.result),
                      str(conf.result)[:120] if conf.ok else conf.message)

    if not a.keep:
        lab.step("clean up")
        lab.drop_endpoint(SITE)
        lab.drop_service(NAME, ENV)
    return lab.finish()


if __name__ == "__main__":
    run(main)
