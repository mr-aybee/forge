#!/usr/bin/env python3
"""17 · Environments, placement and CI (see the .md).

D17 one document, two environments: `--environment` and `overrides`.
D18 placement by node tags and replicas; a node left out is cleaned up.
D19 a CI job deploys with a restricted service-account token: only its own
service, and it cannot approve.
D20 the console (manual checklist in the .md).
"""

from __future__ import annotations

import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from labkit import ci_deploy  # noqa: E402
from labkit.lab import Lab, image_doc, parser, run  # noqa: E402

IMAGE = "hashicorp/http-echo:1.0"


def echo_doc() -> dict:
    d = image_doc("s17-echo", "staging", IMAGE, ports=["8083:5678"], entrypoint="/http-echo -text=staging",
                  strategy={"type": "recreate"}, placement={"nodes": ["lab-c"]},
                  health={"startupTimeout": "3m", "stabilizationPeriod": "5s", "checks": {}})
    d["spec"]["overrides"] = {"prod": {"template": {"ports": ["8084:5678"], "entrypoint": "/http-echo -text=prod"}}}
    return d


def py_doc(selector: dict, replicas: int | None, fx: dict, mock: str) -> dict:
    a = fx["hello-py"]["v1"]["tar"]
    placement = {"nodeSelector": selector}
    if replicas:
        placement["replicas"] = replicas
    return {
        "apiVersion": "forge/v1", "kind": "Deployment", "metadata": {"name": "s17-py", "environment": "dev"},
        "spec": {
            "runtime": "systemd", "artifact": {"archive": {"url": mock + a["path"], "sha256": a["sha256"]}},
            "template": {"command": "python3 app.py", "listenPort": 8090}, "placement": placement,
            "strategy": {"type": "recreate"}, "health": {"startupTimeout": "2m", "stabilizationPeriod": "5s"},
        },
    }


def main() -> int:
    a = parser(__doc__).parse_args()
    lab = Lab(a, "17-deploy-environments-placement-ci")
    fx, mock = lab.state["fixtures"], lab.state["mock"]

    if lab.wanted("D17") or lab.wanted("D19"):
        lab.step("D17 one document, two environments")
        r = lab.apply(echo_doc())
        lab.check("D17", "staging release", r.ok, r.message)
        r = lab.apply(echo_doc(), environment="prod")
        lab.check("D17", "the same document in prod (--environment)", r.ok, r.message)
        s = lab.node_sh(a.docker_node, "curl -fsS http://127.0.0.1:8083/; curl -fsS http://127.0.0.1:8084/")
        lab.check("D17", "each environment got its own values", s.ok and str(s.result).split() == ["staging", "prod"], str(s.result))

    if lab.wanted("D18"):
        lab.step("D18 placement by tags")
        r = lab.apply(py_doc({"runtime": "native"}, 2, fx, mock))
        app = lab.service("s17-py", "dev")
        nodes = sorted(i["server_id"] for i in lab.api_ok("GET", f"/api/v1/services/{app['id']}")["instances"])  # type: ignore[index]
        want = sorted([lab.node_id("lab-b"), lab.node_id("lab-c")])
        lab.check("D18", "runtime:native, 2 replicas → lab-b and lab-c", r.ok and nodes == want, r.message or str(nodes))
        r = lab.apply(py_doc({"zone": "a"}, None, fx, mock))
        lab.check("D18", "zone:a → lab-b only", r.ok, r.message)
        end = time.time() + 300
        gone = False
        while time.time() < end and not gone:
            inst = [i["server_id"] for i in lab.api_ok("GET", f"/api/v1/services/{app['id']}")["instances"]]  # type: ignore[index]
            gone = inst == [lab.node_id("lab-b")]
            time.sleep(3)
        lab.check("D18", "the service is removed from lab-c", gone)
        u = lab.node_sh("lab-c", "systemctl list-units --all 'forge-app-dev-s17-py*' --no-legend | wc -l")
        lab.check("D18", "no unit left on lab-c", str(u.result).strip() == "0", str(u.result))

    if lab.wanted("D19"):
        lab.step("D19 CI with a restricted token")
        echo = lab.service("s17-echo", "staging")
        lab.forge("access", "profiles", "create", "CI deployer", "--action", "view", "--action", "deploy", "--action", "job.create")
        scope = f"service:{echo['id']}"  # type: ignore[index]
        restriction = [{"scope": scope, "actions": ["view", "deploy", "job.create"]}]
        tok = lab.service_account("ci-s17", "Lab CI for s17-echo",
                                  {"name": f"run-{int(time.time())}", "expires_days": 1, "restrictions": restriction})
        lab.forge("access", "assignments", "add", "svc-ci-s17", "CI deployer", scope, "--yes")
        token = tok["secret"]  # type: ignore[index]
        lab.check("D19", "the token is restricted", bool(tok.get("restrictions")), json.dumps(tok.get("restrictions")))  # type: ignore[union-attr]
        template = json.dumps(echo_doc()).replace("-text=staging", "-text=${TAG}")
        code = ci_deploy.deploy(template, {"TAG": "ci-v2"}, token, forge=lab.forge_bin)
        lab.check("D19", "the CI job deploys its service (exit 0)", code == 0, f"exit {code}")
        s = lab.node_sh(a.docker_node, "curl -fsS http://127.0.0.1:8083/")
        lab.check("D19", "the new release answers", "ci-v2" in str(s.result), str(s.result))
        other = lab.forge_as(token, "deploy", "plan", "-f", str(lab.doc_file(py_doc({"zone": "a"}, None, fx, mock))))
        lab.check("D19", "it cannot deploy another service (exit 4)", other.code == 4, other.message[:120])

    print("D20 the console: follow the checklist in 17-deploy-environments-placement-ci.md")

    if not a.keep:
        lab.step("clean up")
        for name, env in (("s17-echo", "staging"), ("s17-echo", "prod"), ("s17-py", "dev")):
            lab.drop_service(name, env)
    return lab.finish()


if __name__ == "__main__":
    run(main)
