#!/usr/bin/env python3
"""Container platform lab: a control plane and N disposable "VMs", each with
its own Docker Engine, set up end to end through the `forge` CLI.

    python3 scripts/lab/swarm/lab.py up            # 3 nodes: lab-a (manager), lab-b, lab-c
    python3 scripts/lab/swarm/lab.py up --nodes 5 --managers 3
    python3 scripts/lab/swarm/lab.py status
    eval "$(python3 scripts/lab/swarm/lab.py env)"  # then use `forge …` directly
    python3 scripts/lab/swarm/lab.py forge -- docker images lab-a
    python3 scripts/lab/swarm/lab.py down [--reset]

Build first: `cargo build --profile perf -p forge-server -p forge-agent -p forge-cli`
and `(cd src/console && npm ci && npm run build)`.

The nodes are privileged containers (Docker in Docker): use this lab only on
a development machine. Lab credentials live in scripts/lab/swarm/.lab/
(git-ignored, mode 0600). Only the bootstrap (first sign-in and one API key)
talks HTTP directly; everything else is a `forge` command, printed as it runs.
"""

from __future__ import annotations

import argparse
import hashlib
import http.cookiejar
import json
import os
import secrets
import shutil
import string
import subprocess
import sys
import time
import urllib.parse
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
LAB = HERE / ".lab"
NET = "forge-swarm-lab"
CP = "forge-swarm-cp"
NODE_IMAGE = "forge-lab-swarm-node:local"
CP_IMAGE = "forge-lab-swarm-cp:local"
CLUSTER = "lab"
ADMIN = "admin@lab.example"
CREDS = LAB / "credentials.json"


def node_names(n: int) -> list[str]:
    if not 1 <= n <= 26:
        sys.exit("--nodes: 1 to 26 (lab-a … lab-z)")
    return [f"lab-{c}" for c in string.ascii_lowercase[:n]]


def run(cmd: list[str], *, check: bool = True, capture: bool = False, env: dict | None = None, stdin: str | None = None):
    return subprocess.run(cmd, check=check, text=True, capture_output=capture, env=env, input=stdin)


def docker(*args: str, check: bool = True, capture: bool = True) -> str:
    r = run(["docker", *args], check=check, capture=capture)
    return (r.stdout or "").strip()


def exists(kind: str, name: str) -> bool:
    return run(["docker", kind, "inspect", name], check=False, capture=True).returncode == 0


def say(msg: str) -> None:
    print(f"\033[1m== {msg}\033[0m", flush=True)


# ------------------------------------------------------------------ binaries


def binaries(profile: str) -> Path:
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "src" / "forge" / "target")) / profile
    missing = [b for b in ("forge-server", "forge-agent", "forge-agent-helper", "forge") if not (target / b).exists()]
    if missing:
        sys.exit(f"missing in {target}: {', '.join(missing)}; build with "
                 f"`cargo build --profile {profile} -p forge-server -p forge-agent -p forge-cli`")
    return target


def stage(target: Path) -> None:
    """Copies the binaries into .lab/ (copy then rename: a running lab keeps its files)."""
    for sub in ("bin", "dist"):
        (LAB / sub).mkdir(parents=True, exist_ok=True)
    put = [(target / "forge-server", LAB / "bin/forge-server"),
           (target / "forge-agent", LAB / "dist/forge-agent-linux-x86_64"),
           (target / "forge-agent-helper", LAB / "dist/forge-agent-helper-linux-x86_64")]
    for extra in ("forge-plugin-runner", "forge-analytics-worker"):
        if (target / extra).exists():
            put.append((target / extra, LAB / "bin" / extra))
    for src, dst in put:
        tmp = dst.with_suffix(".new")
        shutil.copy2(src, tmp)
        tmp.replace(dst)


# ------------------------------------------------------------------ control plane


def config(port: int) -> str:
    runner = "plugin_runner = \"/opt/forge/bin/forge-plugin-runner\"\n" if (LAB / "bin/forge-plugin-runner").exists() else ""
    return f"""data_dir = "/var/lib/forge"
api_listen = "0.0.0.0:8090"
agent_listen = "0.0.0.0:3843"
public_url = "http://localhost:{port}"
node_public_url = "http://{CP}:8090"
agent_public_url = "https://{CP}:3843"
agent_hostnames = ["{CP}", "localhost", "127.0.0.1"]
cookie_secure = false
ui_dir = "/opt/forge/ui"
agent_dist_dir = "/opt/forge/dist"
{runner}
[security]
# Lab only: no authenticator app. Forge refuses this for a non-loopback public URL.
require_mfa = false
"""


def creds() -> dict:
    if CREDS.exists():
        return json.loads(CREDS.read_text())
    c = {"admin": ADMIN, "password": secrets.token_urlsafe(18)}
    save(c)
    return c


def save(c: dict) -> None:
    LAB.mkdir(parents=True, exist_ok=True)
    fd = os.open(CREDS, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        json.dump(c, f, indent=2)


def start_cp(port: int, c: dict, bind: str = "127.0.0.1") -> None:
    (LAB / "data").mkdir(parents=True, exist_ok=True)
    (LAB / "forge.toml").write_text(config(port))
    ui = ROOT / "src/console/dist"
    if not (ui / "index.html").exists():
        print("warning: src/console/dist is missing; the console will not load (npm run build in src/console)")
        ui.mkdir(parents=True, exist_ok=True)
    mounts = ["-v", f"{LAB}/bin:/opt/forge/bin:ro", "-v", f"{LAB}/dist:/opt/forge/dist:ro",
              "-v", f"{ui}:/opt/forge/ui:ro", "-v", f"{LAB}/forge.toml:/etc/forge/forge.toml:ro",
              "-v", f"{LAB}/data:/var/lib/forge"]
    if exists("container", CP):
        docker("start", CP)
        return
    if not (LAB / "data/forge.db").exists():
        # Before the server starts: it loads role bindings at startup.
        run(["docker", "run", "--rm", *mounts, "-e", f"FORGE_ADMIN_PASSWORD={c['password']}", CP_IMAGE,
             "/opt/forge/bin/forge-server", "--config", "/etc/forge/forge.toml", "init", "--email", c["admin"]],
            capture=True)
    docker("run", "-d", "--name", CP, "--hostname", CP, "--network", NET, "-p", f"{bind}:{port}:8090",
           *mounts, CP_IMAGE, "/opt/forge/bin/forge-server", "--config", "/etc/forge/forge.toml", "serve")


def wait_health(url: str) -> None:
    for _ in range(120):
        try:
            with urllib.request.urlopen(f"{url}/api/health", timeout=2):
                return
        except OSError:
            time.sleep(0.5)
    print(docker("logs", "--tail", "40", CP, check=False))
    sys.exit("the control plane did not become healthy")


def bootstrap_token(url: str, c: dict) -> str:
    """Signs in once (solving the login challenge) and mints a 7-day API key."""
    jar = http.cookiejar.CookieJar()
    op = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))

    def call(method: str, path: str, body: dict | None = None, headers: dict | None = None) -> dict:
        req = urllib.request.Request(f"{url}{path}", method=method, headers={"Content-Type": "application/json", **(headers or {})},
                                     data=json.dumps(body).encode() if body is not None else None)
        with op.open(req, timeout=30) as r:
            return json.loads(r.read() or b"{}")

    body = {"login": c["admin"], "password": c["password"]}
    ch = call("GET", "/api/v1/auth/challenge?login=" + urllib.parse.quote(c["admin"]))
    if ch.get("required"):
        salt, bits, n = bytes.fromhex(ch["salt"]), int(ch["difficulty"]), 0
        while int.from_bytes(hashlib.sha256(salt + str(n).encode()).digest(), "big") >> (256 - bits):
            n += 1
        body.update(challenge_id=ch["id"], nonce=str(n))
    login = call("POST", "/api/v1/auth/login", body)
    key = call("POST", "/api/v1/api-keys", {"name": "swarm-lab", "expires_days": 7},
               {"X-CSRF-Token": login["csrf_token"]})
    return key["secret"]


# ------------------------------------------------------------------ forge CLI


class Forge:
    def __init__(self, exe: Path, url: str, token: str):
        self.exe, self.env = exe, {**os.environ, "FORGE_SERVER": url, "FORGE_TOKEN": token, "FORGE_PAGER": "cat"}

    def __call__(self, *args: str, quiet: bool = False, check: bool = True) -> object:
        shown = " ".join(a if " " not in a else repr(a) for a in args)
        if not quiet:
            print(f"   $ forge {shown}", flush=True)
        r = run([str(self.exe), "--output", "json", "--non-interactive", *args], check=False, capture=True, env=self.env)
        if r.returncode != 0:
            if check:
                sys.exit(f"forge {shown} failed ({r.returncode}):\n{r.stderr or r.stdout}")
            return None
        if not r.stdout.strip():
            return None
        doc = json.loads(r.stdout)
        # `--output json` wraps answers in the `forge.cli/v1` envelope.
        return doc.get("result", doc) if isinstance(doc, dict) else doc


def online(forge: Forge, names: list[str]) -> dict[str, str]:
    """Node name → id for the named nodes that are online."""
    rows = forge("nodes", "list", quiet=True, check=False) or []
    return {n["hostname"]: n["id"] for n in rows
            if n.get("hostname") in names and n.get("status") == "online"}


def up(a: argparse.Namespace) -> None:
    names = node_names(a.nodes)
    if not 1 <= a.managers <= a.nodes:
        sys.exit("--managers: 1 to --nodes")
    target = binaries(a.profile)
    url = f"http://localhost:{a.port}"
    say("Staging binaries")
    stage(target)
    say("Building the node and control plane images")
    docker("build", "-q", "-t", NODE_IMAGE, "-f", str(HERE / "Dockerfile.node"), str(HERE), capture=False)
    docker("build", "-q", "-t", CP_IMAGE, "-f", str(HERE / "Dockerfile.cp"), str(HERE), capture=False)
    if not exists("network", NET):
        docker("network", "create", NET)
    say(f"Control plane on {url}")
    c = creds()
    start_cp(a.port, c, a.bind)
    wait_health(url)
    if "token" not in c:
        c["token"] = bootstrap_token(url, c)
        save(c)
    forge = Forge(target / "forge", url, c["token"])

    say("Cluster `lab` in Swarm mode")
    rows = forge("clusters", "list", quiet=True) or []
    if not any(x.get("name") == CLUSTER for x in rows):
        spec = LAB / "cluster.json"
        spec.write_text(json.dumps({"name": CLUSTER, "description": "Container platform lab", "mode": "swarm"}))
        forge("clusters", "create", "--file", str(spec))

    say(f"Nodes: {', '.join(names)}")
    for name in names:
        if exists("container", name):
            docker("start", name)
            continue
        tok = forge("nodes", "enroll-token", "--cluster", CLUSTER, "--hostname", name, "--ttl-minutes", "30")
        docker("run", "-d", "--name", name, "--hostname", name, "--network", NET, "--privileged",
               "--cpus", "1", "--memory", "1536m", "-v", f"{name}-docker:/var/lib/docker",
               "-v", f"{name}-agent:/var/lib/forge-agent", "-e", f"FORGE_URL=http://{CP}:8090",
               "-e", f"FORGE_ENROLL_TOKEN={tok['token']}", NODE_IMAGE)
    deadline = time.monotonic() + 240
    while (ids := online(forge, names)).keys() != set(names):
        if time.monotonic() > deadline:
            sys.exit(f"not online after 4 minutes: {sorted(set(names) - ids.keys())}; see `docker logs <node>`")
        time.sleep(3)
    print(f"   all {len(names)} nodes online")

    say("Swarm: first manager, then the rest")
    swarm = forge("clusters", "swarm", "status", CLUSTER, quiet=True) or {}
    members = {m.get("server_id") for m in swarm.get("members", [])}
    for i, name in enumerate(names):
        if ids[name] in members:
            continue
        role = "manager" if i < a.managers else "worker"
        forge("clusters", "swarm", "join", CLUSTER, name, "--role", role, "--wait")
    forge("clusters", "swarm", "status", CLUSTER)
    print(f"""
Lab is up.
  Console:  {url}   sign-in: {c['admin']} (password in {CREDS})
  CLI:      eval "$(python3 {Path(__file__).relative_to(ROOT)} env)" then e.g.
            forge clusters swarm status lab
            forge docker images lab-a
            forge stacks templates
""")


def status(a: argparse.Namespace) -> None:
    for name in [CP, *node_names(26)]:
        if exists("container", name):
            print(f"{name:20} {docker('inspect', '-f', '{{.State.Status}}', name)}")
    if CREDS.exists() and "token" in creds():
        forge = Forge(binaries(a.profile) / "forge", f"http://localhost:{a.port}", creds()["token"])
        print(json.dumps(forge("clusters", "swarm", "status", CLUSTER, quiet=True, check=False), indent=2))


def env(a: argparse.Namespace) -> None:
    c = creds()
    if "token" not in c:
        sys.exit("the lab is not up yet: run `lab.py up`")
    print(f"export FORGE_SERVER=http://localhost:{a.port}")
    print(f"export FORGE_TOKEN={c['token']}")
    print(f"export PATH={binaries(a.profile)}:$PATH")


def forge_cmd(a: argparse.Namespace) -> None:
    c = creds()
    e = {**os.environ, "FORGE_SERVER": f"http://localhost:{a.port}", "FORGE_TOKEN": c.get("token", "")}
    sys.exit(run([str(binaries(a.profile) / "forge"), *a.args], check=False, env=e).returncode)


def down(a: argparse.Namespace) -> None:
    names = [n for n in [CP, *node_names(26)] if exists("container", n)]
    if a.reset:
        if names:
            docker("rm", "-f", *names)
        vols = [v for v in docker("volume", "ls", "-q").split() if v.startswith("lab-") and v.endswith(("-docker", "-agent"))]
        if vols:
            docker("volume", "rm", *vols)
        if exists("network", NET):
            docker("network", "rm", NET)
        shutil.rmtree(LAB / "data", ignore_errors=True)
        for f in (CREDS, LAB / "forge.toml", LAB / "cluster.json"):
            f.unlink(missing_ok=True)
        print("lab removed (staged binaries kept in .lab/bin and .lab/dist)")
    else:
        if names:
            docker("stop", *names)
        print("lab stopped; `lab.py up` starts it again")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--profile", default="perf", help="cargo profile the binaries come from (default perf)")
    ap.add_argument("--port", type=int, default=8095, help="console port (default 8095)")
    ap.add_argument("--bind", default="127.0.0.1",
                    help="host address the console listens on; 0.0.0.0 for every interface (default 127.0.0.1)")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("up", help="start (or resume) the lab")
    p.add_argument("--nodes", type=int, default=3)
    p.add_argument("--managers", type=int, default=1, help="Swarm managers; use 3 for a fault-tolerant quorum")
    p.set_defaults(fn=up)
    sub.add_parser("status", help="containers and the swarm").set_defaults(fn=status)
    sub.add_parser("env", help="print FORGE_SERVER / FORGE_TOKEN exports").set_defaults(fn=env)
    p = sub.add_parser("forge", help="run a forge command against the lab")
    p.add_argument("args", nargs=argparse.REMAINDER)
    p.set_defaults(fn=forge_cmd)
    p = sub.add_parser("down", help="stop the lab")
    p.add_argument("--reset", action="store_true", help="also delete containers, volumes and lab data")
    p.set_defaults(fn=down)
    a = ap.parse_args()
    if getattr(a, "args", None) and a.args[0] == "--":
        a.args = a.args[1:]
    a.fn(a)


if __name__ == "__main__":
    main()
