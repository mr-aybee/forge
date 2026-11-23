#!/usr/bin/env python3
"""14 · Canary releases and their gates (see the .md).

D05 a canary shifts traffic in weighted steps (20 %, 50 %, 100 %); the edge
splits requests between the two copies by those weights.
D06 a canary of a release that answers 500 trips its error-rate gate: the
deployment fails, the new copy is removed and traffic stays on the old one.
"""

from __future__ import annotations

import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from labkit.lab import Lab, Traffic, image_doc, parser, run  # noqa: E402

ENV, NAME, SITE, DOMAIN = "prod", "s14-echo", "s14-echo", "s14.lab.local"
IMAGE = "hashicorp/http-echo:1.0"
HTTP_CHECK = {"readiness": [{"name": "http", "probe": {"type": "http", "port": 8082, "path": "/"}}]}


def doc(args: str, strategy: dict, checks: dict | None = None) -> dict:
    health = {"startupTimeout": "3m", "stabilizationPeriod": "5s", "checks": HTTP_CHECK if checks is None else checks}
    return image_doc(NAME, ENV, IMAGE, ports=["8082:5678"], entrypoint=f"/http-echo {args}", strategy=strategy,
                     health=health, placement={"nodes": ["lab-c"]}, traffic={"drainTimeout": "5s"})


def main() -> int:
    a = parser(__doc__).parse_args()
    lab = Lab(a, "14-deploy-canary")
    edge_node = lab.state.get("edge_node", "lab-a")

    lab.step("release 1 and the endpoint")
    r = lab.apply(doc("-text=v1", {"type": "recreate"}))
    lab.check("D05", "release 1", r.ok, r.message)
    app = lab.service(NAME, ENV)
    lab.endpoint(SITE, DOMAIN, app["id"], 8082, [lab.node_id(edge_node)])  # type: ignore[index]
    t = Traffic(a.edge, DOMAIN)

    if lab.wanted("D05"):
        lab.step("D05 canary 20 % → 50 % → 100 %")
        steps = [{"weight": 20, "pause": "90s"}, {"weight": 50, "pause": "30s"}, {"weight": 100}]
        r = lab.apply(doc("-text=v2", {"type": "canary", "steps": steps}), wait=False)
        dep = (r.result or {}).get("id") if r.ok else None
        lab.check("D05", "the canary starts", bool(dep), r.message)
        if dep:
            # Wait for the 20 % step (the first route), then measure the split.
            end = time.time() + 300
            while time.time() < end and "canary at 20%" not in " ".join(e["message"] for e in lab.deployment(dep)["events"]):
                time.sleep(2)
            split = t.sample(400)
            share = split.get("v2", 0) / max(1, sum(split.values()))
            lab.check("D05", "about 20 % of requests reach v2", 0.10 <= share <= 0.32, f"{split}")
            conf = lab.node_sh(edge_node, f"cat /etc/nginx/forge/sites/{SITE}.conf")
            lab.check("D05", "the edge has the weights", conf.ok and "weight=20" in str(conf.result), conf.message)
            d = lab.wait_state(dep, {"succeeded", "failed", "rolled_back"}, timeout=a.timeout * 60)
            lab.check("D05", "the canary completes", d["state"] == "succeeded", d.get("error") or d["state"])
            lab.check("D05", "all traffic on v2", t.sample(20) == {"v2": 20}, str(t.sample(5)))

    if lab.wanted("D06"):
        lab.step("D06 a canary that answers 500 trips its gate")
        steps = [{"weight": 30, "pause": "60s", "analysis": {"maxErrorRate": 5, "minRequests": 20}}, {"weight": 100}]
        load = Traffic(a.edge, DOMAIN, interval=0.05).start()
        r = lab.apply(doc("-text=boom -status-code=500", {"type": "canary", "steps": steps}, checks={}))
        load.stop()
        lab.check("D06", "the rollout fails (exit 10)", r.code == 10, f"exit {r.code}: {r.message[:120]}")
        # The canary took traffic, so the previous release is rolled out again.
        end = time.time() + a.timeout * 60
        while time.time() < end:
            hist = lab.must("deploy", "history", NAME, "-n", ENV) or []
            if hist and hist[0]["state"] in {"succeeded", "failed"} and hist[0]["kind"] == "auto_rollback":
                break
            time.sleep(3)
        failed = next((h for h in hist if h["state"] == "rolled_back"), {})
        lab.check("D06", "the gate names the error rate", "requests failed (at most 5%)" in str(failed.get("error")), str(failed.get("error")))
        lab.check("D06", "an automatic rollback restores release 2",
                  bool(hist) and hist[0]["kind"] == "auto_rollback" and hist[0]["state"] == "succeeded" and hist[0]["release_number"] == 2,
                  str(hist[0] if hist else None))
        slots = lab.slots(NAME, ENV)["slots"]
        lab.check("D06", "one copy serves", len([s for s in slots if s["weight"] > 0]) == 1, str(slots))
        lab.check("D06", "traffic is back on v2 only", t.sample(20) == {"v2": 20}, str(t.sample(5)))

    if not a.keep:
        lab.step("clean up")
        lab.drop_endpoint(SITE)
        lab.drop_service(NAME, ENV)
    return lab.finish()


if __name__ == "__main__":
    run(main)
