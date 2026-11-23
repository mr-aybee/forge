#!/usr/bin/env python3
"""13 · Blue-green with approval; pause, resume and abort (see the .md).

D04 blue-green: the new release runs next to the old on every node, waits for
an approval (its author may not approve; another principal does), then all
traffic moves at once.
D14 control: pause holds a rollout, resume continues it; abort stops one and
removes the copy that never took traffic.
"""

from __future__ import annotations

import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from labkit.lab import Lab, Traffic, image_doc, parser, run  # noqa: E402

ENV, NAME, SITE, DOMAIN = "staging", "s13-echo", "s13-echo", "s13.lab.local"
IMAGE = "hashicorp/http-echo:1.0"


def doc(text: str, strategy: dict, stabilization: str = "5s") -> dict:
    health = {
        "startupTimeout": "3m",
        "stabilizationPeriod": stabilization,
        "checks": {"readiness": [{"name": "http", "probe": {"type": "http", "port": 8081, "path": "/"}}]},
    }
    return image_doc(NAME, ENV, IMAGE, ports=["8081:5678"], entrypoint=f"/http-echo -text={text}", strategy=strategy,
                     health=health, placement={"nodes": ["lab-c"]}, traffic={"drainTimeout": "5s"})


def texts(t: Traffic, n: int = 20) -> dict[str, int]:
    return t.sample(n)


def main() -> int:
    a = parser(__doc__).parse_args()
    lab = Lab(a, "13-deploy-blue-green-approval")
    edge_node = lab.state.get("edge_node", "lab-a")
    approver = lab.approver_token()

    lab.step("release 1 and the endpoint")
    r = lab.apply(doc("v1", {"type": "recreate"}))
    lab.check("D04", "release 1", r.ok, r.message)
    app = lab.service(NAME, ENV)
    lab.endpoint(SITE, DOMAIN, app["id"], 8081, [lab.node_id(edge_node)])  # type: ignore[index]
    t = Traffic(a.edge, DOMAIN)
    lab.check("D04", "the endpoint answers v1", texts(t) == {"v1": 20}, str(t.bodies))

    if lab.wanted("D04"):
        lab.step("D04 blue-green waiting for approval")
        r = lab.apply(doc("v2", {"type": "blue-green", "approval": {"required": True, "timeout": "10m"}}), wait=False)
        dep = (r.result or {}).get("id") if r.ok else None
        lab.check("D04", "the deployment starts", bool(dep), r.message)
        if dep:
            d = lab.wait_state(dep, {"awaiting_approval", "failed"}, timeout=300)
            lab.check("D04", "it waits for an approval", d["state"] == "awaiting_approval", d["state"])
            lab.check("D04", "traffic still on v1 while it waits", texts(t) == {"v1": 20}, str(t.bodies))
            own = lab.forge("deploy", "approve", dep)
            lab.check("D04", "its author cannot approve it (exit 4)", own.code == 4, own.message[:120])
            ok = lab.forge_as(approver, "deploy", "approve", dep)
            lab.check("D04", "another principal approves it", ok.ok, ok.message)
            d = lab.wait_state(dep, {"succeeded", "failed", "rolled_back", "aborted"}, timeout=a.timeout * 60)
            lab.check("D04", "release 2 went live", d["state"] == "succeeded", d.get("error") or d["state"])
            lab.check("D04", "all traffic on v2 at once", texts(t) == {"v2": 20}, str(t.sample(5)))

    if lab.wanted("D14"):
        lab.step("D14 pause and resume")
        r = lab.apply(doc("v3", {"type": "rolling", "replacement": "start-first"}, stabilization="40s"), wait=False)
        dep = (r.result or {}).get("id") if r.ok else None
        lab.check("D14", "a slow rollout starts", bool(dep), r.message)
        if dep:
            time.sleep(5)
            lab.forge("deploy", "pause", dep)
            d = lab.wait_state(dep, {"paused", "succeeded", "failed"}, timeout=300)
            lab.check("D14", "pause holds it before its next step", d["state"] == "paused", d["state"])
            step = d["step"]
            time.sleep(15)
            lab.check("D14", "nothing moves while paused", lab.deployment(dep)["step"] == step)
            lab.forge("deploy", "resume", dep)
            d = lab.wait_state(dep, {"succeeded", "failed", "rolled_back"}, timeout=a.timeout * 60)
            lab.check("D14", "resume finishes it", d["state"] == "succeeded", d.get("error") or d["state"])

        lab.step("D14 abort before traffic moves")
        r = lab.apply(doc("v4", {"type": "rolling", "replacement": "start-first"}, stabilization="60s"), wait=False)
        dep = (r.result or {}).get("id") if r.ok else None
        if dep:
            time.sleep(10)
            lab.forge("deploy", "abort", dep)
            d = lab.wait_state(dep, {"aborted", "failed", "succeeded"}, timeout=300)
            lab.check("D14", "abort ends it", d["state"] == "aborted", d["state"])
            slots = lab.slots(NAME, ENV)["slots"]
            lab.check("D14", "the aborted copy is gone, one copy serves", len(slots) == 1 and slots[0]["weight"] == 100, str(slots))
            lab.check("D14", "traffic stays on v3", texts(t) == {"v3": 20}, str(t.sample(5)))

    if not a.keep:
        lab.step("clean up")
        lab.drop_endpoint(SITE)
        lab.drop_service(NAME, ENV)
    return lab.finish()


if __name__ == "__main__":
    run(main)
