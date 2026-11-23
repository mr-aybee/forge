#!/usr/bin/env python3
"""10 · Deployment lab setup (see 10-deploy-setup.md).

Prepares everything scenarios 11–17 use, then (with --checkpoint, on the
Hyper-V host) saves it as checkpoint `deploy-ready`:

1. the mock GitHub (HTTPS, lab CA) with the test artifacts;
2. the lab CA trusted on the worker nodes; python3, git, curl, unzip there;
3. nginx on the edge node;
4. environments dev, staging and prod in cluster `lab`;
5. node tags (zone, docker);
6. the approver: a service account that may approve deployments.

    python 10-deploy-setup.py [--mock-bind 10.50.0.1] [--checkpoint]
"""

from __future__ import annotations

import json
import os
import ssl
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from labkit import fixtures, mock_github  # noqa: E402
from labkit.lab import SETUP_TESTING, Lab, LabError, parser, run  # noqa: E402

LAB_DIR = SETUP_TESTING / ".lab"


def start_mock(lab: Lab, bind: str, port: int) -> str:
    base = f"https://{bind}:{port}"
    ca, _, _ = mock_github.ensure_certs(bind)
    ctx = ssl.create_default_context(cafile=str(ca))
    try:
        urllib.request.urlopen(f"{base}/healthz", context=ctx, timeout=3).read()
        lab.step(f"mock GitHub already answers at {base}")
        return base
    except OSError:
        pass
    log = LAB_DIR / "mock-github.log"
    flags = subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.DETACHED_PROCESS if os.name == "nt" else 0
    subprocess.Popen(
        [sys.executable, str(Path(mock_github.__file__)), "--bind", bind, "--port", str(port)],
        stdout=log.open("ab"), stderr=subprocess.STDOUT, creationflags=flags, start_new_session=os.name != "nt",
    )
    for _ in range(30):
        try:
            urllib.request.urlopen(f"{base}/healthz", context=ctx, timeout=2).read()
            lab.step(f"mock GitHub started at {base} (log {log})")
            return base
        except OSError:
            time.sleep(0.5)
    raise LabError(f"the mock GitHub did not start; see {log}")


def open_firewall(port: int) -> None:
    if os.name != "nt":
        return
    name = f"Forge lab mock GitHub {port}"
    subprocess.run(["netsh", "advfirewall", "firewall", "delete", "rule", f"name={name}"], capture_output=True)
    r = subprocess.run(["netsh", "advfirewall", "firewall", "add", "rule", f"name={name}", "dir=in", "action=allow",
                        "protocol=TCP", f"localport={port}"], capture_output=True, text=True)
    if r.returncode != 0:
        print(f"  note: could not add firewall rule '{name}' (needs an admin shell); add it once by hand")


def main() -> int:
    p = parser(__doc__)
    p.add_argument("--mock-bind", default="10.50.0.1", help="address the nodes reach this machine at")
    p.add_argument("--mock-port", type=int, default=8443)
    p.add_argument("--edge-node", default="lab-a")
    p.add_argument("--checkpoint", action="store_true", help="create checkpoint deploy-ready at the end (Hyper-V host)")
    a = p.parse_args()
    lab = Lab(a, "10-deploy-setup")

    lab.step("signed in, nodes online")
    who = lab.forge("auth", "whoami")
    lab.check("S01", "signed in", who.ok, who.message)
    nodes = lab.nodes()
    workers = a.nodes.split(",")
    for n in [a.edge_node, *workers]:
        lab.check("S01", f"{n} online", nodes.get(n, {}).get("status") == "online", str(nodes.get(n, {}).get("status")))
    lab.cluster_id("lab")

    lab.step("artifacts and the mock GitHub")
    fx = fixtures.build()
    open_firewall(a.mock_port)
    base = start_mock(lab, a.mock_bind, a.mock_port)
    ca_pem = (LAB_DIR / "ca" / "lab-ca.crt").read_text()

    lab.step("worker nodes: trust the lab CA, tools for native artifacts")
    for n in workers:
        r = lab.node_sh(n, "cat > /usr/local/share/ca-certificates/forge-lab.crt && update-ca-certificates >/dev/null", stdin=ca_pem)
        lab.check("S02", f"{n} trusts the lab CA", r.ok, r.message)
        r = lab.node_sh(n, f"curl -fsS {base}/healthz")
        lab.check("S02", f"{n} reaches the mock over HTTPS", r.ok, r.message)
        missing = lab.node_sh(n, "for t in python3 git curl unzip runuser; do command -v $t >/dev/null || echo $t; done")
        if missing.ok and str(missing.result or "").strip():
            lab.node_sh(n, "DEBIAN_FRONTEND=noninteractive apt-get install -y -q python3 git curl unzip >/dev/null", timeout=900)
        r = lab.node_sh(n, "python3 --version && git --version && unzip -v | head -1")
        lab.check("S02", f"{n} has python3, git, unzip", r.ok, r.message)

    lab.step("nginx on the edge")
    lab.check("S03", "nginx installed on the edge", lab.install_package("nginx", "stable", [a.edge_node]))
    r = lab.node_sh(a.edge_node, "systemctl is-active nginx")
    lab.check("S03", "nginx runs on the edge", r.ok, r.message)

    lab.step("environments and node tags")
    envs = {e: lab.environment(e) for e in ("dev", "staging", "prod")}
    lab.check("S04", "environments dev, staging, prod", all(envs.values()))
    tags = {"lab-b": ["zone:a", "runtime:native"], "lab-c": ["zone:b", "runtime:native", "docker:yes"]}
    for n, t in tags.items():
        if n in nodes:
            r = lab.api("PUT", f"/api/v1/tags/server/{nodes[n]['id']}", {"tags": t})
            lab.check("S05", f"{n} tagged {', '.join(t)}", r.ok, r.message)

    lab.step("the approver (a second principal)")
    secret_file = LAB_DIR / "deploy-approver.token"
    # A token from an earlier lab is useless after a rebuild: always a new one.
    secret_file.unlink(missing_ok=True)
    lab.forge("access", "profiles", "create", "Deploy approver", "--action", "view", "--action", "deploy.approve")
    tok = lab.service_account("deploy-approver", "Approves lab deployments", {"name": "lab", "expires_days": 30})
    # Access follows service → environment → project → workspace; a
    # cluster is placement, not a parent (D-056). So the approver is given
    # the `lab` project, which holds dev/staging/prod.
    project = next(g["id"] for g in lab.must("projects", "list", "--all") or [] if g["name"] == "lab")
    lab.must("access", "assignments", "add", "svc-deploy-approver", "Deploy approver", f"project:{project}", "--yes")
    secret_file.write_text(tok["secret"])
    if os.name != "nt":
        secret_file.chmod(0o600)
    lab.check("S06", "approver token stored in .lab/", secret_file.exists())

    lab.save_state(
        mock=base,
        fixtures=fx,
        edge=a.edge,
        edge_node=a.edge_node,
        nodes={n: nodes[n]["id"] for n in nodes},
        environments=envs,
        approver_token_file=str(secret_file),
    )
    if a.checkpoint:
        r = subprocess.run([sys.executable, str(SETUP_TESTING / "vm_lab.py"), "checkpoint", "deploy-ready"])
        lab.check("S07", "checkpoint deploy-ready created", r.returncode == 0)
    return lab.finish()


if __name__ == "__main__":
    run(main)
