"""From checkpoint `clean` to a lab the scenarios can use (Hyper-V host only).

    python labkit/bootstrap.py [--no-rebuild] [--tag <release>]

1. `vm_lab.py reset`, `prepare`, `install-server`: restore `clean` on lab-a,
   lab-b and lab-c, prepare them and install the latest release from the
   local feed on lab-a (`--no-rebuild` skips this when the lab is fresh);
2. MFA stays off on the lab server (turned off again if an install turned
   it on), then `add-nodes --with-server` onboards lab-a, lab-b and lab-c;
3. cluster `lab` with every node in it;
4. an API key for the scripts, written to `.lab/forge-admin.token` (git
   ignored; never printed). Scripts read it with the server URL from
   `.lab/lab-client.json`, so they need no `forge auth login` — on Windows
   or in WSL alike.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
SETUP = HERE.parent.parent
LAB_DIR = SETUP / ".lab"
TOKEN_FILE = LAB_DIR / "forge-admin.token"
CLIENT_FILE = LAB_DIR / "lab-client.json"


def vm_lab():
    """The `vm_lab.py` module (its Lab and Api classes)."""
    spec = importlib.util.spec_from_file_location("vm_lab", SETUP / "vm_lab.py")
    mod = importlib.util.module_from_spec(spec)  # type: ignore[arg-type]
    spec.loader.exec_module(mod)  # type: ignore[union-attr]
    return mod


def step(*args: str) -> None:
    r = subprocess.run([sys.executable, str(SETUP / "vm_lab.py"), *args])
    if r.returncode != 0:
        raise SystemExit(f"vm_lab.py {' '.join(args)} failed")


def rebuild(tag: str | None) -> None:
    """Restore `clean`, prepare and install the server (nodes are onboarded after MFA is checked)."""
    step("reset")
    step("prepare")
    step("install-server", *(["--tag", tag] if tag else []))


def mfa_off(lab) -> None:
    """Keeps MFA off on the lab server (the lab's own choice; never on a real install)."""
    cli = "/opt/forge/forge-server --config /etc/forge/forge.toml mfa"
    r = lab.ssh("lab-a", f"{cli} status 2>&1", sudo=True, check=False)
    if "off" in (r.stdout or "").lower():
        return
    print("MFA is on after the install: turning it off (lab only)")
    lab.ssh("lab-a", f"{cli} off && systemctl restart forge-server", sudo=True)
    for _ in range(30):
        if lab.ssh("lab-a", "curl -fsS http://127.0.0.1:3880/api/health", check=False).returncode == 0:
            return
        time.sleep(2)


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--no-rebuild", action="store_true")
    p.add_argument("--tag", help="release tag to install (default: latest on the feed)")
    a = p.parse_args()
    if not a.no_rebuild:
        rebuild(a.tag)
    vl = vm_lab()
    lab = vl.Lab()
    mfa_off(lab)
    # lab-a is a node too: the scenarios use it as the edge (and as a worker in 01).
    step("add-nodes", "--with-server")
    api = lab.api()
    servers = api.call("GET", "/api/v1/servers") or []
    clusters = {c["name"]: c["id"] for c in api.call("GET", "/api/v1/clusters") or []}
    if "lab" not in clusters:
        clusters["lab"] = api.call("POST", "/api/v1/clusters", {"name": "lab", "description": "Scenario lab"})["id"]
    for s in servers:
        if s.get("cluster_id") != clusters["lab"]:
            api.call("PATCH", f"/api/v1/servers/{s['id']}", {"cluster_id": clusters["lab"]})
    key = api.call("POST", "/api/v1/api-keys", {"name": f"lab-scenarios-{int(time.time())}", "expires_days": 7})
    LAB_DIR.mkdir(parents=True, exist_ok=True)
    TOKEN_FILE.write_text(key["secret"])
    if os.name != "nt":
        TOKEN_FILE.chmod(0o600)
    CLIENT_FILE.write_text(json.dumps({"server": lab.console, "edge": "10.50.0.10"}, indent=2))
    names = ", ".join(sorted(f"{s['hostname']}={s['status']}" for s in servers))
    print(f"lab ready: cluster lab ({names}); API key in {TOKEN_FILE.name}")


if __name__ == "__main__":
    main()
