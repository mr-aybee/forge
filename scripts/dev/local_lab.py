#!/usr/bin/env python3
"""Runs forge-server on this machine (from the working tree) for the lab, with
the lab VMs enrolled as agents. No install, no release, no feed: rebuild and
restart straight from the cargo targets. Works on Linux and Windows.

On Windows the lab VMs reach the host on 10.50.0.1 (ForgeLab Hyper-V
switch) and the Linux agent is built in WSL (--distro). On Linux the agent is
built natively (needs the x86_64-unknown-linux-musl target); pass --host-ip
with the address the VMs reach this machine on.

Actions (run in order the first time):
  setup    Uses the fixed Forge ports (55130 API, 55131 agents, 55132 UI;
           saved in <data-dir>/ports.json), writes <data-dir>/forge.toml,
           opens the two ports in the firewall for 10.50.0.0/24 (Windows
           only, asks for admin), creates the admin user.
  build    Builds forge-server and forge-agent + forge-agent-helper (Linux
           musl) with ONE fresh version stamp <next version>-dev.<yyyyMMdd.HHmmss>
           (for Cargo version 0.1.0: 0.1.1-dev.…), and
           copies the agents into <data-dir>/dist where the server hands them
           to nodes.
  run      Starts the server in this terminal. Ctrl-C stops it.
  dev      build + run (the normal loop after a code change).
  enroll   Prints the one-line agent install command for a VM.
  status   Ports, URLs, version of the built server and agents.

After `dev`, the agents on the VMs are one version behind: update them from
the UI (Administration -> Forge updates -> Agents, "Update agents") or with
`forge agents update --all --wait --yes`. Same version stamp = same build.

Examples:
  python3 scripts/dev/local_lab.py setup
  python3 scripts/dev/local_lab.py dev
  python3 scripts/dev/local_lab.py enroll --node lab-b
"""
from __future__ import annotations

import argparse
import datetime as dt
import getpass
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

from dist_manifest import write_manifest
from _common import EXE, REPO, WINDOWS, build_env, cyan, dev_version, die, green, read_env_file, run

DEFAULT_DATA = Path(r"D:\forge-dev") if WINDOWS else Path.home() / ".local" / "share" / "forge-dev"
MUSL = "x86_64-unknown-linux-musl"

CONFIG = """# Local lab server (scripts/dev/local_lab.py). Development only.
data_dir = "{data}/data"
api_listen = "0.0.0.0:{api}"
agent_listen = "0.0.0.0:{agent}"
public_url = "http://{host}:{api}"
node_public_url = "http://{host}:{api}"
agent_public_url = "https://{host}:{agent}"
agent_hostnames = ["{host}", "localhost", "127.0.0.1"]
agent_dist_dir = "{data}/dist"
ui_dir = "{repo}/src/ui/dist"
environment = "development"
# Plain HTTP: a Secure cookie is dropped by the browser and sign-in loops.
cookie_secure = false

[security]
require_mfa = false

[update]
enabled = false
"""

# Quick lab build of the agent: opt-level 1, no LTO, own target dir.
AGENT_BUILD = """set -e
export FORGE_BUILD_VERSION='{stamp}'
export CARGO_TARGET_DIR="$HOME/.cache/forge-target-quick"
export CARGO_PROFILE_RELEASE_OPT_LEVEL=1 CARGO_PROFILE_RELEASE_LTO=off CARGO_PROFILE_RELEASE_CODEGEN_UNITS=256 CARGO_PROFILE_RELEASE_INCREMENTAL=true
command -v sccache >/dev/null && [ -z "${{RUSTC_WRAPPER:-}}" ] && export RUSTC_WRAPPER=sccache
command -v ld.lld >/dev/null && export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS='-C link-arg=-fuse-ld=lld'
cargo build --release --target {musl} -p forge-agent
out="$CARGO_TARGET_DIR/{musl}/release"
mkdir -p '{dist}'
cp "$out/forge-agent" '{dist}/forge-agent-linux-x86_64'
cp "$out/forge-agent-helper" '{dist}/forge-agent-helper-linux-x86_64'
"""


def wsl_path(p: Path) -> str:
    s = str(p)
    return "/mnt/" + s[0].lower() + s[2:].replace("\\", "/")


class LocalLab:
    def __init__(self, a: argparse.Environment) -> None:
        self.a = a
        self.data: Path = a.data_dir.resolve()
        self.cfg = self.data / "forge.toml"
        self.ports_file = self.data / "ports.json"
        self.dist = self.data / "dist"
        self.server = REPO / "target" / "debug" / f"forge-server{EXE}"
        self.dist.mkdir(parents=True, exist_ok=True)

    def ports(self) -> dict[str, int]:
        if self.ports_file.exists():
            return json.loads(self.ports_file.read_text(encoding="utf-8"))
        p = {"api": 55130, "agent": 55131, "ui": 55132}
        self.ports_file.write_text(json.dumps(p, indent=2), encoding="utf-8")
        return p

    def version(self) -> str:
        f = self.data / "version.txt"
        return f.read_text(encoding="utf-8").strip() if f.exists() else ""

    def write_config(self, p: dict[str, int]) -> None:
        self.cfg.write_text(CONFIG.format(data=self.data.as_posix(), repo=REPO.as_posix(), host=self.a.host_ip,
                                          api=p["api"], agent=p["agent"]), encoding="utf-8")

    def build(self) -> None:
        stamp = dev_version()
        print(cyan(f"==> version {stamp}"))
        env = build_env()
        env["FORGE_BUILD_VERSION"] = stamp
        print("==> forge-server")
        run(["cargo", "build", "-p", "forge-server"], cwd=REPO, env=env, what="forge-server build")

        print("==> forge-agent + helper (Linux musl" + (", WSL)" if WINDOWS else ")"))
        dist = wsl_path(self.dist) if WINDOWS else self.dist.as_posix()
        script = self.data / "build-agent.sh"
        # Through a file: quoting a multi-line script on the wsl command line loses $vars.
        script.write_bytes(AGENT_BUILD.format(stamp=stamp, musl=MUSL, dist=dist).encode())
        if WINDOWS:
            rc = subprocess.call(["wsl", "-d", self.a.distro, "--cd", str(REPO), "--", "bash", "-l", wsl_path(script)])
        else:
            rc = subprocess.call(["bash", str(script)], cwd=REPO)
        if rc:
            die("agent build failed", rc)
        # The server offers the agents as the version in MANIFEST.json.
        write_manifest(self.dist, stamp)
        (self.data / "version.txt").write_text(stamp, encoding="utf-8")

        if not (REPO / "src" / "ui" / "dist" / "index.html").exists():
            print("==> UI (first time)")
            run(["npm", "ci"], cwd=REPO / "src" / "ui")
            run(["npm", "run", "build"], cwd=REPO / "src" / "ui")

    def serve(self) -> None:
        if not self.server.exists():
            die("build first: local_lab.py build")
        p, h = self.ports(), self.a.host_ip
        print(green(f"forge-server {self.version()}"))
        print(f"  console  http://{h}:{p['api']}   (or http://localhost:{p['api']})")
        print(f"  agents   https://{h}:{p['agent']}")
        print(f"  UI hot reload: cd src/ui; npm run dev  ->  http://localhost:{p['ui']} (proxies /api to :{p['api']})")
        try:
            subprocess.call([str(self.server), "--config", str(self.cfg), "serve"])
        except KeyboardInterrupt:
            pass

    def open_firewall(self, p: dict[str, int]) -> None:
        if not WINDOWS:
            print(f"firewall: allow TCP {p['api']},{p['agent']} from the lab network yourself if a firewall is active "
                  f"(for example: sudo ufw allow from 10.50.0.0/24 to any port {p['api']},{p['agent']} proto tcp)")
            return
        rule = f"Forge local lab ({p['api']},{p['agent']})"
        ps = shutil.which("pwsh") or "powershell"
        exists = subprocess.call([ps, "-NoProfile", "-Command",
                                  f"if (Get-NetFirewallRule -DisplayName '{rule}' -ErrorAction SilentlyContinue) {{ exit 0 }} else {{ exit 1 }}"])
        if exists != 0:
            cmd = (f"New-NetFirewallRule -DisplayName '{rule}' -Direction Inbound -Protocol TCP "
                   f"-LocalPort {p['api']},{p['agent']} -RemoteAddress 10.50.0.0/24 -Action Allow | Out-Null")
            subprocess.call([ps, "-NoProfile", "-Command",
                             f"Start-Process {ps} -Verb RunAs -Wait -ArgumentList '-NoProfile','-Command',\"{cmd}\""])
            print(f"firewall: {rule}")

    def setup(self) -> None:
        p = self.ports()
        self.write_config(p)
        print(f"wrote {self.cfg} (api {p['api']}, agent {p['agent']})")
        self.open_firewall(p)
        if not (self.data / "data" / "forge.db").exists():
            self.build()
            # Reuse the lab admin password (scripts/setup_testing/.lab/forge-admin.env) when present.
            lab_env = REPO / "scripts" / "setup_testing" / ".lab" / "forge-admin.env"
            user, pw = "admin", os.environ.get("FORGE_ADMIN_PASSWORD", "")
            if lab_env.exists():
                kv = read_env_file(lab_env)
                user = kv.get("FORGE_ADMIN_USER") or user
                pw = kv.get("FORGE_ADMIN_PASSWORD") or pw
            if not pw:
                pw = getpass.getpass(f"Password for the local admin '{user}' (12+ characters): ")
            rc = subprocess.call([str(self.server), "--config", str(self.cfg), "init", "--username", user],
                                 env=dict(os.environ, FORGE_ADMIN_PASSWORD=pw))
            if rc:
                die("forge-server init failed", rc)
        print(f"next: local_lab.py run   (then create cluster '{self.a.cluster}' and enroll the VMs: "
              "local_lab.py enroll --node lab-a)")

    def enroll(self) -> None:
        p, h, a = self.ports(), self.a.host_ip, self.a
        print("1) Token (signed in with the forge CLI against this server):")
        print(f"   forge contexts add local --server http://127.0.0.1:{p['api']}; forge auth login")
        print(f'   forge clusters create --file cluster.json   # once: {{"name":"{a.cluster}","abbr":"LAB"}}')
        print(f"   forge nodes enroll-token --cluster {a.cluster} --hostname {a.node}")
        print("   (or in the UI: Nodes -> Add node)")
        print(f"2) On {a.node} (paste the token at the prompt):")
        print("   read -rs FORGE_ENROLL_TOKEN && export FORGE_ENROLL_TOKEN")
        print(f"   curl -fsSL 'http://{h}:{p['api']}/install.sh?terminal_account=forge-ops' "
              "| sudo --preserve-env=FORGE_ENROLL_TOKEN sh")

    def status(self) -> None:
        p = self.ports()
        print(f"config   {self.cfg}")
        print(f"ports    api {p['api']}, agent {p['agent']}")
        print(f"console  http://{self.a.host_ip}:{p['api']}")
        print(f"version  {self.version()}")
        for f in sorted(self.dist.iterdir()):
            st = f.stat()
            print(f"  {f.name:<40} {st.st_size:>12}  {dt.datetime.fromtimestamp(st.st_mtime):%Y-%m-%d %H:%M}")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("action", nargs="?", default="status", choices=["setup", "build", "run", "dev", "enroll", "status"])
    ap.add_argument("--data-dir", type=Path, default=DEFAULT_DATA)
    ap.add_argument("--host-ip", default="10.50.0.1")
    ap.add_argument("--distro", default="Ubuntu-24.04", help="WSL distribution (Windows only)")
    ap.add_argument("--node", default="<node-name>")
    ap.add_argument("--cluster", default="lab")
    a = ap.parse_args()

    lab = LocalLab(a)
    if a.action == "setup":
        lab.setup()
    elif a.action == "build":
        lab.build()
    elif a.action == "run":
        lab.serve()
    elif a.action == "dev":
        lab.build()
        lab.serve()
    elif a.action == "enroll":
        lab.enroll()
    else:
        lab.status()


if __name__ == "__main__":
    main()
