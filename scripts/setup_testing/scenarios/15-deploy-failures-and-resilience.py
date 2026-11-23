#!/usr/bin/env python3
"""15 · Failures and resilience (see the .md).

D07 a release that never becomes ready: the gate times out, the new copy is
removed, traffic never moved (no failed request).
D12 an archive whose SHA-256 does not match, and D11b a Git commit that does
not exist: the node refuses before anything starts.
D16 the control plane restarts in the middle of a gate: the rollout picks up
the step it was on; a node whose agent drops out for 40 s delays the rollout
but does not break it.
"""

from __future__ import annotations

import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from labkit.lab import Lab, Traffic, image_doc, parser, run  # noqa: E402

ENV, NAME, SITE, DOMAIN = "staging", "s15-web", "s15-web", "s15.lab.local"
WHOAMI = "traefik/whoami:v1.10.1"
CHECKS = {"readiness": [{"name": "http", "probe": {"type": "http", "port": 8080, "path": "/health"}}]}


def web(image: str, strategy: dict, *, entrypoint: str = "", startup: str = "3m", stable: str = "5s") -> dict:
    d = image_doc(NAME, ENV, image, ports=["8080:80"], entrypoint=entrypoint, strategy=strategy,
                  health={"startupTimeout": startup, "stabilizationPeriod": stable, "checks": CHECKS},
                  placement={"nodes": ["lab-c"]}, traffic={"drainTimeout": "5s"})
    return d


def native(name: str, artifact: dict) -> dict:
    return {
        "apiVersion": "forge/v1", "kind": "Deployment",
        "metadata": {"name": name, "environment": "dev"},
        "spec": {
            "runtime": "systemd", "artifact": artifact,
            "template": {"command": "python3 app.py", "listenPort": 8090},
            "placement": {"nodes": ["lab-b"]},
            "strategy": {"type": "recreate"},
            "health": {"startupTimeout": "2m", "stabilizationPeriod": "5s"},
        },
    }


def main() -> int:
    a = parser(__doc__).parse_args()
    lab = Lab(a, "15-deploy-failures-and-resilience")
    edge_node = lab.state.get("edge_node", "lab-a")
    mock, fx = lab.state.get("mock", ""), lab.state.get("fixtures", {})

    lab.step("release 1 and the endpoint")
    r = lab.apply(web(WHOAMI, {"type": "recreate"}))
    lab.check("D07", "release 1", r.ok, r.message)
    app = lab.service(NAME, ENV)
    lab.endpoint(SITE, DOMAIN, app["id"], 8080, [lab.node_id(edge_node)])  # type: ignore[index]

    if lab.wanted("D07"):
        lab.step("D07 a release that never becomes ready")
        t = Traffic(a.edge, DOMAIN, "/health").start()
        # nginx with a command that exits: the container never stays up.
        r = lab.apply(web("nginx:1.27-alpine", {"type": "rolling", "replacement": "start-first"},
                          entrypoint="/bin/sh -c exit", startup="45s"))
        t.stop()
        hist = lab.must("deploy", "history", NAME, "-n", ENV) or []
        lab.check("D07", "the rollout fails (exit 10)", r.code == 10, r.message[:120])
        lab.check("D07", "the gate explains why", "did not become ready" in str(hist[0].get("error")), str(hist[0].get("error")))
        lab.check("D07", "no automatic rollback (no traffic moved)", hist[0]["state"] == "failed", hist[0]["state"])
        slots = lab.slots(NAME, ENV)["slots"]
        lab.check("D07", "only the old copy is left", len(slots) == 1 and slots[0]["slot"] == "", str(slots))
        lab.check("D07", "no failed request meanwhile", t.failures == 0, f"{t.total} requests, {t.statuses}")

    if lab.wanted("D12"):
        lab.step("D12 an archive whose checksum does not match")
        broken = fx["hello-py"]["broken"]["tar"]
        r = lab.apply(native("s15-py", {"archive": {"url": mock + broken["path"], "sha256": broken["sha256"]}}))
        hist = lab.must("deploy", "history", "s15-py", "-n", "dev") or []
        err = str(hist[0].get("error")) if hist else r.message
        lab.check("D12", "refused on the node", r.code == 10 and "SHA-256" in err, err[:160])
        ps = lab.node_sh("lab-b", "systemctl list-units --all 'forge-app-dev-s15-py*' --no-legend | wc -l")
        lab.check("D12", "nothing was started", str(ps.result).strip() in {"0", ""}, str(ps.result))

        lab.step("D11b a Git commit that does not exist")
        repo = mock + fx["git"]["repo_path"]
        r = lab.apply(native("s15-git", {"git": {"repo": repo, "commit": "0" * 40}}))
        hist = lab.must("deploy", "history", "s15-git", "-n", "dev") or []
        err = str(hist[0].get("error")) if hist else r.message
        lab.check("D11b", "the fetch fails with a clear message", r.code == 10 and "git fetch" in err, err[:160])

    if lab.wanted("D16"):
        lab.step("D16 the control plane restarts during a gate")
        t = Traffic(a.edge, DOMAIN, "/health").start()
        r = lab.apply(web("traefik/whoami:v1.11.0", {"type": "rolling", "replacement": "start-first"}, stable="60s"), wait=False)
        dep = (r.result or {}).get("id") if r.ok else None
        lab.check("D16", "a slow rollout starts", bool(dep), r.message)
        if dep:
            time.sleep(20)
            lab.node_sh(edge_node, "systemd-run --on-active=2s systemctl restart forge-server")
            time.sleep(15)
            d = lab.wait_state(dep, {"succeeded", "failed", "rolled_back", "aborted"}, timeout=a.timeout * 60)
            t.stop()
            lab.check("D16", "it resumes and completes", d["state"] == "succeeded", d.get("error") or d["state"])
            lab.check("D16", "no failed request through the edge", t.failures == 0, f"{t.total} requests, {t.statuses}")

        lab.step("D16 the node's agent drops out for 40 s")
        lab.node_sh(a.docker_node, "systemd-run --on-active=3s /bin/sh -c 'systemctl stop forge-agent; sleep 40; systemctl start forge-agent'")
        time.sleep(5)
        r = lab.apply(web(WHOAMI, {"type": "rolling", "replacement": "start-first"}))
        lab.check("D16", "the rollout waits for the node and completes", r.ok, r.message)

    if not a.keep:
        lab.step("clean up")
        lab.drop_endpoint(SITE)
        lab.drop_service(NAME, ENV)
        for n in ("s15-py", "s15-git"):
            lab.drop_service(n, "dev")
    return lab.finish()


if __name__ == "__main__":
    run(main)
