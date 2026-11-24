#!/usr/bin/env python3
"""Hyper-V VM test lab: rebuild, install, enroll and inspect from Windows.

Three Debian 13 VMs on the ForgeLab switch (see MOP.md):

  lab-a  10.50.0.10  Forge server (installed from the local release feed)
  lab-b  10.50.0.11  node
  lab-c  10.50.0.12  node + Docker Engine (containers, App Store tests)

Settings and the VM login live in scripts/setup_testing/.lab/vm-lab.env
(git-ignored; `init` writes a template). The Forge admin login written by the
installer is kept in .lab/forge-admin.env. Passwords are never printed and
never put on a command line: SSH reads them through SSH_ASKPASS, sudo from
stdin.

    py scripts/setup_testing/vm_lab.py init              # write .lab/vm-lab.env template
    py scripts/setup_testing/vm_lab.py status            # VMs, services, versions
    py scripts/setup_testing/vm_lab.py rebuild           # reset + prepare + install + add nodes
    py scripts/setup_testing/vm_lab.py rebuild --publish # also build and publish a release first
    py scripts/setup_testing/vm_lab.py reset             # restore the Hyper-V checkpoint (admin)
    py scripts/setup_testing/vm_lab.py reset --checkpoint deploy-ready
    py scripts/setup_testing/vm_lab.py checkpoint deploy-ready   # create/replace a checkpoint on every VM
    py scripts/setup_testing/vm_lab.py prepare           # sudo check, Docker on the docker node
    py scripts/setup_testing/vm_lab.py install-server    # install/upgrade Forge on lab-a from the feed
    py scripts/setup_testing/vm_lab.py add-nodes         # enroll lab-b and lab-c as nodes
    py scripts/setup_testing/vm_lab.py run lab-b -- uname -a
    py scripts/setup_testing/vm_lab.py sudo lab-c -- docker ps

Needs: Windows OpenSSH (built in), the feed server running
(`py scripts/setup_testing/feed_server.py`) for install-server, and Hyper-V
rights (admin PowerShell, or membership of "Hyper-V Administrators") for reset.
Standard library only.
"""

from __future__ import annotations

import argparse
import hashlib
import http.cookiejar
import json
import os
import shlex
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
LAB = HERE / ".lab"
ENV_FILE = LAB / "vm-lab.env"
ADMIN_FILE = LAB / "forge-admin.env"
KNOWN_HOSTS = LAB / "known_hosts"
ASKPASS = LAB / "askpass.cmd"
OWNER, REPO_NAME = "orbyntech", "forge"

TEMPLATE = """\
# Hyper-V lab settings for scripts/setup_testing/vm_lab.py (git-ignored).
FORGE_LAB_USER=forge
FORGE_LAB_PASSWORD=
# name=ip; the first is the Forge server.
FORGE_LAB_SERVER=lab-a=10.50.0.10
FORGE_LAB_NODES=lab-b=10.50.0.11,lab-c=10.50.0.12
# Node that runs Docker Engine for container and App Store tests.
FORGE_LAB_DOCKER_NODE=lab-c
# Hyper-V VM names, in the same order as server then nodes.
FORGE_LAB_VMS=LAB-A,LAB-B,LAB-C
FORGE_LAB_CHECKPOINT=clean
# Local release feed (feed_server.py) as the VMs reach it.
FORGE_LAB_FEED=http://10.50.0.1:8000
"""

# ------------------------------------------------------------------ output
TTY = sys.stdout.isatty() and not os.environ.get("NO_COLOR")
def _c(code: str, s: str) -> str:
    return f"\033[{code}m{s}\033[0m" if TTY else s
def ok(msg: str) -> None:
    print(f"  {_c('32', '✔')} {msg}")
def warn(msg: str) -> None:
    print(f"  {_c('33', '▲')} {msg}")
def bad(msg: str) -> None:
    print(f"  {_c('31', '✘')} {msg}")
def head(msg: str) -> None:
    print(f"\n{_c('1', msg)}")
def die(msg: str) -> "None":
    bad(msg)
    sys.exit(1)


# ------------------------------------------------------------------ config
def load_env(path: Path) -> dict[str, str]:
    out: dict[str, str] = {}
    if path.is_file():
        for line in path.read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                k, v = line.split("=", 1)
                out[k.strip()] = v.strip()
    return out


class Lab:
    def __init__(self) -> None:
        if not ENV_FILE.is_file():
            die(f"{ENV_FILE} is missing: run `py {Path(__file__).name} init` and fill in the password.")
        e = load_env(ENV_FILE)
        self.user = e.get("FORGE_LAB_USER", "forge")
        self.password = e.get("FORGE_LAB_PASSWORD", "")
        if not self.password:
            die(f"set FORGE_LAB_PASSWORD in {ENV_FILE}")
        if any(c in self.password for c in "%&|<>^!\"\r\n"):
            die("FORGE_LAB_PASSWORD contains characters the Windows askpass helper cannot pass (% & | < > ^ ! \")")
        name, ip = e.get("FORGE_LAB_SERVER", "lab-a=10.50.0.10").split("=")
        self.server = (name, ip)
        self.nodes = [tuple(x.split("=")) for x in e.get("FORGE_LAB_NODES", "").split(",") if "=" in x]
        self.docker_node = e.get("FORGE_LAB_DOCKER_NODE", "")
        self.vms = [v.strip() for v in e.get("FORGE_LAB_VMS", "LAB-A,LAB-B,LAB-C").split(",") if v.strip()]
        self.checkpoint = e.get("FORGE_LAB_CHECKPOINT", "clean")
        self.feed = e.get("FORGE_LAB_FEED", "http://10.50.0.1:8000").rstrip("/")
        self.hosts = dict([self.server, *self.nodes])
        self.console = f"http://{self.server[1]}:3880"

    # -------------------------------------------------------------- ssh
    def _ssh_env(self) -> dict[str, str]:
        LAB.mkdir(exist_ok=True)
        if not ASKPASS.is_file():
            ASKPASS.write_text("@echo off\r\necho %FORGE_LAB_PASSWORD%\r\n", encoding="ascii")
        env = dict(os.environ)
        env.update(
            FORGE_LAB_PASSWORD=self.password,
            SSH_ASKPASS=str(ASKPASS),
            SSH_ASKPASS_REQUIRE="force",
            DISPLAY=env.get("DISPLAY", "forge-lab"),
        )
        return env

    @staticmethod
    def _ssh_bin() -> str:
        # Windows' OpenSSH honours SSH_ASKPASS on Windows; prefer it over Git's.
        win = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32" / "OpenSSH" / "ssh.exe"
        return str(win) if win.is_file() else (shutil.which("ssh") or "ssh")

    def ip(self, name: str) -> str:
        if name in self.hosts:
            return self.hosts[name]
        if name in self.hosts.values():
            return name
        die(f"unknown VM {name!r} (known: {', '.join(self.hosts)})")
        return ""

    def ssh(self, name: str, command: str, *, sudo: bool = False, timeout: int = 900, check: bool = True,
            capture: bool = True, stdin_line: str = "") -> subprocess.CompletedProcess:
        """Runs `command` on a VM. With sudo, the password goes to sudo on stdin.
        `stdin_line` follows on stdin (after sudo's line), for secrets the
        command reads with `read`, so they never appear on a command line."""
        remote = f"sudo -k -S -p '' bash -c {shlex.quote(command)}" if sudo else command
        args = [
            self._ssh_bin(), "-o", "StrictHostKeyChecking=accept-new", "-o", f"UserKnownHostsFile={KNOWN_HOSTS}",
            "-o", "ConnectTimeout=8", "-o", "ServerAliveInterval=15", "-o", "LogLevel=ERROR",
            "-o", "PreferredAuthentications=publickey,password", f"{self.user}@{self.ip(name)}", remote,
        ]
        r = subprocess.run(
            args,
            input=((self.password + "\n") if sudo else "") + (stdin_line + "\n" if stdin_line else ""),
            text=True, env=self._ssh_env(),
            capture_output=capture, timeout=timeout, encoding="utf-8", errors="replace",
        )
        if check and r.returncode != 0:
            tail = (r.stderr or r.stdout or "").strip().splitlines()[-15:] if capture else []
            for line in tail:
                print(f"      {line}")
            die(f"{name}: command failed (exit {r.returncode})")
        return r

    def wait_ssh(self, name: str, deadline_s: int = 300) -> None:
        end = time.monotonic() + deadline_s
        while time.monotonic() < end:
            try:
                if self.ssh(name, "true", check=False, timeout=15).returncode == 0:
                    return
            except subprocess.TimeoutExpired:
                pass
            time.sleep(3)
        die(f"{name} did not answer on SSH within {deadline_s}s")

    # -------------------------------------------------------------- forge api
    def admin(self) -> tuple[str, str]:
        a = load_env(ADMIN_FILE)
        if not a.get("FORGE_ADMIN_PASSWORD"):
            die(f"no Forge admin login in {ADMIN_FILE}: run install-server on a fresh server (or write it by hand)")
        return a.get("FORGE_ADMIN_USER", "admin"), a["FORGE_ADMIN_PASSWORD"]

    def api(self) -> "Api":
        user, pw = self.admin()
        return Api(self.console, user, pw)


def login_challenge(base: str, login: str) -> dict:
    """Solves the server's login challenge (proof of work, D-064), if any."""
    url = f"{base}/api/v1/auth/challenge?login={urllib.parse.quote(login)}"
    try:
        with urllib.request.urlopen(url, timeout=30) as r:
            c = json.loads(r.read())
    except (urllib.error.URLError, ValueError):
        return {}  # older servers have no challenge
    if not c.get("required"):
        return {}
    salt, bits, n = bytes.fromhex(c["salt"]), int(c["difficulty"]), 0
    while int.from_bytes(hashlib.sha256(salt + str(n).encode()).digest(), "big") >> (256 - bits):
        n += 1
    return {"challenge_id": c["id"], "nonce": str(n)}


class Api:
    def __init__(self, base: str, login: str, password: str) -> None:
        self.base = base
        self.opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
        self.csrf = None
        body = {"login": login, "password": password, **login_challenge(base, login)}
        self.csrf = self.call("POST", "/api/v1/auth/login", body)["csrf_token"]

    def call(self, method: str, path: str, body=None):
        req = urllib.request.Request(self.base + path, method=method)
        req.add_header("Content-Type", "application/json")
        if self.csrf:
            req.add_header("X-CSRF-Token", self.csrf)
        data = json.dumps(body).encode() if body is not None else None
        try:
            with self.opener.open(req, data=data, timeout=30) as r:
                raw = r.read()
        except urllib.error.HTTPError as e:
            die(f"{method} {path}: HTTP {e.code} {e.read().decode(errors='replace')[:300]}")
        return json.loads(raw) if raw else None


def http_json(url: str, timeout: int = 10):
    with urllib.request.urlopen(url, timeout=timeout) as r:
        return json.loads(r.read())


def wait_for(what: str, fn, deadline_s: int):
    end = time.monotonic() + deadline_s
    last = None
    while time.monotonic() < end:
        try:
            last = fn()
            if last:
                return last
        except Exception:  # noqa: BLE001 - polled until the deadline
            pass
        time.sleep(2)
    die(f"timed out after {deadline_s}s waiting for {what}")


# ------------------------------------------------------------------ commands
def cmd_init(_: argparse.Environment) -> None:
    LAB.mkdir(exist_ok=True)
    if ENV_FILE.exists():
        ok(f"{ENV_FILE} already exists (left unchanged)")
        return
    ENV_FILE.write_text(TEMPLATE, encoding="utf-8")
    ok(f"wrote {ENV_FILE}; set FORGE_LAB_PASSWORD there")


def cmd_status(_: argparse.Environment) -> None:
    lab = Lab()
    head("VMs")
    probe = (
        "printf '%s|' \"$(hostname)\" \"$(. /etc/os-release; echo $PRETTY_NAME)\" "
        "\"$(free -m | awk '/Mem:/{print $3\"/\"$2\" MB\"}')\" \"$(df -h / | awk 'NR==2{print $3\"/\"$2}')\" "
        "\"$(systemctl is-active forge-server 2>/dev/null)\" \"$(systemctl is-active forge-agent 2>/dev/null)\" "
        "\"$(systemctl is-active docker 2>/dev/null)\""
    )
    for name in lab.hosts:
        try:
            r = lab.ssh(name, probe, check=False, timeout=20)
        except subprocess.TimeoutExpired:
            bad(f"{name:7} {lab.hosts[name]:15} SSH timed out")
            continue
        if r.returncode != 0:
            bad(f"{name:7} {lab.hosts[name]:15} unreachable over SSH")
            continue
        host, os_name, mem, disk, srv, agent, docker = (r.stdout.strip().split("|") + [""] * 7)[:7]
        services = " ".join(
            f"{n}={s}" for n, s in (("server", srv), ("agent", agent), ("docker", docker)) if s and s != "inactive"
        ) or "no Forge services"
        ok(f"{name:7} {lab.hosts[name]:15} {os_name} · mem {mem} · disk {disk} · {services}")
    head("Forge")
    try:
        v = http_json(f"{lab.console}/api/version")
        ok(f"server {v['version']} (build {v.get('build') or '?'}) at {lab.console}")
    except Exception as e:  # noqa: BLE001
        warn(f"server not answering at {lab.console} ({e.__class__.__name__})")
        return
    try:
        latest = http_json(f"{lab.feed}/repos/{OWNER}/{REPO_NAME}/releases/latest")["tag_name"]
        ok(f"feed latest {latest} at {lab.feed}")
    except Exception:  # noqa: BLE001
        warn(f"feed not reachable at {lab.feed} (start feed_server.py)")
    if ADMIN_FILE.is_file():
        for s in lab.api().call("GET", "/api/v1/servers") or []:
            (ok if s.get("status") == "online" else warn)(
                f"node {s.get('hostname'):10} {s.get('status'):8} agent {s.get('agent_version')}"
            )


def cmd_checkpoint(args: argparse.Environment) -> None:
    """Creates (or replaces) checkpoint `name` on every lab VM."""
    lab = Lab()
    head(f"Creating checkpoint {args.name!r} on {', '.join(lab.vms)}")
    vms = ",".join(f"'{v}'" for v in lab.vms)
    ps = (
        f"$ErrorActionPreference='Stop'; foreach ($v in @({vms})) {{ "
        f"Get-VMSnapshot -VMName $v -Name '{args.name}' -ErrorAction SilentlyContinue | Remove-VMSnapshot; "
        f"Checkpoint-VM -Name $v -SnapshotName '{args.name}' }}"
    )
    r = subprocess.run(["powershell", "-NoProfile", "-Command", ps], capture_output=True, text=True)
    if r.returncode != 0:
        for line in (r.stderr or r.stdout).strip().splitlines()[:3]:
            print(f"      {line}")
        die("Hyper-V refused (run from an admin PowerShell, or add yourself to 'Hyper-V Administrators')")
    ok(f"checkpoint {args.name!r} created")


def cmd_reset(args: argparse.Environment) -> None:
    lab = Lab()
    if getattr(args, "checkpoint", None):
        lab.checkpoint = args.checkpoint
    head(f"Restoring checkpoint {lab.checkpoint!r} on {', '.join(lab.vms)}")
    vms = ",".join(f"'{v}'" for v in lab.vms)
    ps = (
        f"$ErrorActionPreference='Stop'; foreach ($v in @({vms})) {{ "
        f"Restore-VMSnapshot -VMName $v -Name '{lab.checkpoint}' -Confirm:$false; "
        f"if ((Get-VM $v).State -ne 'Running') {{ Start-VM $v }} }}"
    )
    r = subprocess.run(["powershell", "-NoProfile", "-Command", ps], capture_output=True, text=True)
    if r.returncode != 0:
        msg = (r.stderr or r.stdout).strip().splitlines()[:3]
        for line in msg:
            print(f"      {line}")
        die("Hyper-V refused (run from an admin PowerShell, or add yourself to 'Hyper-V Administrators')")
    ok("checkpoints restored, VMs starting")
    for name in lab.hosts:
        lab.wait_ssh(name)
        ok(f"{name} is back on SSH")


def cmd_prepare(_: argparse.Environment) -> None:
    lab = Lab()
    head("Preparing VMs")
    for name in lab.hosts:
        lab.wait_ssh(name, 60)
        who = lab.ssh(name, "id -un", sudo=True).stdout.strip()
        if who != "root":
            die(f"{name}: sudo did not give root (got {who!r})")
        ok(f"{name}: SSH and sudo work")
    srv = lab.server[0]
    r = lab.ssh(srv, f"curl -fsS -o /dev/null -w '%{{http_code}}' {lab.feed}/", check=False)
    (ok if r.stdout.strip() == "200" else warn)(f"{srv}: release feed {lab.feed} → {r.stdout.strip() or 'unreachable'}")
    if lab.docker_node:
        d = lab.docker_node
        if lab.ssh(d, "systemctl is-active docker", check=False).stdout.strip() == "active":
            ok(f"{d}: Docker Engine already running")
        else:
            lab.ssh(
                d,
                "export DEBIAN_FRONTEND=noninteractive; apt-get update -qq && "
                "apt-get install -y -qq docker.io >/dev/null && systemctl enable --now docker >/dev/null && "
                f"usermod -aG docker {shlex.quote(lab.user)}",
                sudo=True,
            )
            ok(f"{d}: installed Docker Engine (docker.io) and added {lab.user} to the docker group")
        v = lab.ssh(d, "docker version --format '{{.Server.Version}}'", sudo=True).stdout.strip()
        ok(f"{d}: docker {v}")


def cmd_install_server(args: argparse.Environment) -> None:
    lab = Lab()
    srv, ip = lab.server
    head(f"Installing Forge on {srv}")
    try:
        rel = http_json(f"{lab.feed}/repos/{OWNER}/{REPO_NAME}/releases/" + (f"tags/{args.tag}" if args.tag else "latest"))
    except Exception as e:  # noqa: BLE001
        die(f"release feed {lab.feed} not reachable from Windows ({e}); start feed_server.py")
    tag = rel["tag_name"]
    ver = tag.removeprefix("v")
    pkg = f"forge-{ver}-x86_64-linux"
    pub = (HERE / ".keys" / "release.pub")
    if not pub.is_file():
        die("no dev release key (.keys/release.pub): run publish.py once")
    ok(f"release {tag}")
    url = f"{lab.feed}/{OWNER}/{REPO_NAME}/releases/download/{tag}/{pkg}.tar.gz"
    lab.ssh(srv, f"rm -rf /tmp/forge-pkg && mkdir -p /tmp/forge-pkg && cd /tmp/forge-pkg && "
                 f"curl -fsSL -o {pkg}.tar.gz {shlex.quote(url)} && tar xzf {pkg}.tar.gz")
    ok("package downloaded and unpacked")
    # Rebuilds keep the lab's fixed admin login (.lab/forge-admin.env) when it
    # exists; otherwise the installer generates one and it is saved below.
    fixed = load_env(ADMIN_FILE) if ADMIN_FILE.is_file() else {}
    fixed_pw = fixed.get("FORGE_ADMIN_PASSWORD", "")
    install = (f"cd /tmp/forge-pkg/{pkg} && NO_COLOR=1 ./install.sh --public-url http://{ip}:3880 "
               "--environment development --dev-skip-mfa")  # lab: MFA checks off; production refuses this
    if fixed_pw:
        user = fixed.get("FORGE_ADMIN_USER", "admin")
        install = (f"IFS= read -r FORGE_ADMIN_PASSWORD && export FORGE_ADMIN_PASSWORD && "
                   f"{install} --admin-username {shlex.quote(user)}")
        ok(f"using the fixed admin login from {ADMIN_FILE.relative_to(REPO)} ({user})")
    r = lab.ssh(srv, install, sudo=True, stdin_line=fixed_pw)
    for line in r.stdout.splitlines():
        if line.strip().startswith("Password"):
            continue  # never echo the generated password
        if line.strip():
            print(f"      {line.rstrip()}")
    # Point the updater at the local feed and trust the dev key (lab only).
    feed_url = f"{lab.feed}/repos/{OWNER}/{REPO_NAME}/releases/latest"
    key = pub.read_text(encoding="utf-8").strip()
    lab.ssh(
        srv,
        "f=/etc/forge/forge.toml; sed -i '/^\\[update\\]/,$d' $f; "
        f"printf '[update]\\nenabled = true\\nfeed_url = \"%s\"\\npublic_key = \"%s\"\\n' {shlex.quote(feed_url)} {shlex.quote(key)} >> $f; "
        "systemctl restart forge-server",
        sudo=True,
    )
    ok("updater → local feed, dev key trusted")
    got = wait_for("the server to answer", lambda: http_json(f"{lab.console}/api/version", 5), 90)
    if got["version"] != ver:
        die(f"server reports {got['version']}, expected {ver}")
    ok(f"server {got['version']} answering at {lab.console}")
    # Keep the generated admin login for the API (never printed).
    saved = lab.ssh(srv, "cat /var/lib/forge/initial-admin-password 2>/dev/null || true", sudo=True).stdout
    creds = dict(
        line.split(": ", 1) for line in saved.splitlines() if ": " in line and not line.startswith("#")
    )
    if creds.get("password"):
        LAB.mkdir(exist_ok=True)
        ADMIN_FILE.write_text(
            f"FORGE_ADMIN_USER={creds.get('username', 'admin')}\nFORGE_ADMIN_PASSWORD={creds['password']}\n",
            encoding="utf-8",
        )
        ok(f"admin login saved to {ADMIN_FILE.relative_to(REPO)}")
    elif ADMIN_FILE.is_file():
        ok("existing admin login kept (password was already changed or not generated)")
    else:
        warn(f"no generated password on {srv}: put FORGE_ADMIN_USER/FORGE_ADMIN_PASSWORD in {ADMIN_FILE}")


def cmd_add_nodes(args: argparse.Environment) -> None:
    """Onboards the nodes through Forge itself (assisted SSH onboarding), the
    same path an operator takes in the console: probe, confirm the host key,
    onboard. The host key Forge reports is checked against the key read from
    the node over the lab's own SSH, so a wrong fingerprint is never confirmed.
    """
    lab = Lab()
    head("Onboarding nodes through Forge (SSH)")
    api = lab.api()
    have = {s.get("hostname"): s for s in api.call("GET", "/api/v1/servers") or []}
    jobs = {}
    # --with-server: the server VM is a node too (edge and worker scenarios).
    nodes = ([lab.server] if getattr(args, "with_server", False) else []) + list(lab.nodes)
    for name, ip in nodes:
        if name in have and have[name].get("status") == "online":
            ok(f"{name} already enrolled and online")
            continue
        probe = api.call("POST", "/api/v1/nodes/probe", {"host": ip, "port": 22})
        fp = probe["host_key"]["fingerprint"]
        real = lab.ssh(name, "for f in /etc/ssh/ssh_host_*_key.pub; do ssh-keygen -lf \"$f\"; done",
                       capture=True).stdout
        if fp not in real.split():
            die(f"{name}: Forge saw host key {fp}, which is not one of the node's keys; refusing to onboard")
        ok(f"{name}: host key {fp} confirmed against the node")
        job = api.call("POST", "/api/v1/nodes/onboard", {
            "host": ip, "port": 22, "username": lab.user,
            "host_key_fingerprint": fp,
            "auth": {"type": "password", "password": lab.password},
            "sudo_password": lab.password,
            "keep_credential": False,
            "hostname": name,
        })
        jobs[name] = ((job or {}).get("job") or {}).get("id")
        ok(f"{name}: onboarding job {jobs[name]} started")
    for name, job_id in jobs.items():
        def online(n=name):
            return any(s.get("hostname") == n and s.get("status") == "online" for s in api.call("GET", "/api/v1/servers"))
        wait_for(f"{name} to come online", online, 600)
        ok(f"{name} onboarded by Forge and online")
        if job_id:
            j = api.call("GET", f"/api/v1/jobs/{job_id}")
            ok(f"{name}: job {j.get('status')}")


# Commands enabled on older agents when they are upgraded (as install.sh does).
ALLOWED = "package container app site dns probe storage files access cancel processes process_action names tunnel"


def cmd_upgrade_agents(args: argparse.Environment) -> None:
    """Upgrades the agent and helper on every node to a release from the feed,
    verified against that release's SHA256SUMS. No console login needed."""
    lab = Lab()
    head("Upgrading node agents")
    try:
        rel = http_json(f"{lab.feed}/repos/{OWNER}/{REPO_NAME}/releases/" + (f"tags/{args.tag}" if args.tag else "latest"))
    except Exception as e:  # noqa: BLE001
        die(f"release feed {lab.feed} not reachable ({e})")
    tag = rel["tag_name"]
    sums_path = HERE / "releases" / tag / "SHA256SUMS"
    if not sums_path.is_file():
        die(f"{sums_path} not found (publish the release on this machine)")
    sums = {}
    for line in sums_path.read_text(encoding="utf-8").splitlines():
        parts = line.split()
        if len(parts) == 2:
            sums[parts[1].lstrip("*")] = parts[0]
    base = f"{lab.feed}/{OWNER}/{REPO_NAME}/releases/download/{tag}"
    want_a, want_h = sums.get("forge-agent-linux-x86_64"), sums.get("forge-agent-helper-linux-x86_64")
    if not want_a or not want_h:
        die("the release has no Linux x86_64 agent binaries")
    ok(f"release {tag}")
    script = rf"""set -e
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
curl -fsSL -o "$T/a" {shlex.quote(base + '/forge-agent-linux-x86_64')}
curl -fsSL -o "$T/h" {shlex.quote(base + '/forge-agent-helper-linux-x86_64')}
echo "{want_a}  $T/a" | sha256sum -c - >/dev/null
echo "{want_h}  $T/h" | sha256sum -c - >/dev/null
install -m 0755 "$T/h" /usr/local/bin/forge-agent-helper.new && mv -f /usr/local/bin/forge-agent-helper.new /usr/local/bin/forge-agent-helper
install -m 0755 "$T/a" /usr/local/bin/forge-agent.new && mv -f /usr/local/bin/forge-agent.new /usr/local/bin/forge-agent
CONF=/var/lib/forge-agent/agent.toml
for CMD in {ALLOWED}; do
  if [ -f "$CONF" ] && ! grep -q "\"$CMD\"" "$CONF"; then
    sed -i -e "/^allowed_commands = \[.*\]\$/ s/\]\$/, \"$CMD\"]/" -e "s/^\( *\)\"service_status\",\$/\1\"service_status\",\n\1\"$CMD\",/" "$CONF"
  fi
done
systemctl restart forge-agent-helper.service 2>/dev/null || true
systemctl restart forge-agent.service
sleep 2
/usr/local/bin/forge-agent --version
"""
    for name, _ip in lab.nodes:
        r = lab.ssh(name, script, sudo=True)
        ok(f"{name}: {r.stdout.strip().splitlines()[-1] if r.stdout.strip() else 'upgraded'}")
        def active(n=name):
            return lab.ssh(n, "systemctl is-active forge-agent", check=False).stdout.strip() == "active"
        wait_for(f"{name} agent to run", active, 60)


def cmd_rebuild(args: argparse.Environment) -> None:
    if args.publish:
        head("Publishing a release")
        r = subprocess.run([sys.executable, str(HERE / "publish.py"), *(["--ui"] if args.ui else [])])
        if r.returncode != 0:
            die("publish.py failed")
    if not args.no_reset:
        cmd_reset(args)
    cmd_prepare(args)
    args.tag = None
    cmd_install_server(args)
    cmd_add_nodes(args)
    cmd_status(args)


def cmd_run(args: argparse.Environment, sudo: bool = False) -> None:
    lab = Lab()
    if not args.command:
        die("give a command after --")
    r = lab.ssh(args.vm, shlex.join(args.command), sudo=sudo, check=False, capture=False, timeout=3600)
    sys.exit(r.returncode)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("init").set_defaults(fn=cmd_init)
    sub.add_parser("status").set_defaults(fn=cmd_status)
    p = sub.add_parser("reset")
    p.add_argument("--checkpoint", help="checkpoint to restore (default: FORGE_LAB_CHECKPOINT, `clean`)")
    p.set_defaults(fn=cmd_reset)
    p = sub.add_parser("checkpoint")
    p.add_argument("name", help="checkpoint name, e.g. deploy-ready")
    p.set_defaults(fn=cmd_checkpoint)
    sub.add_parser("prepare").set_defaults(fn=cmd_prepare)
    p = sub.add_parser("install-server")
    p.add_argument("--tag", help="release tag (default: latest on the feed)")
    p.set_defaults(fn=cmd_install_server)
    p = sub.add_parser("add-nodes")
    p.add_argument("--with-server", action="store_true", help="onboard the server VM as a node too")
    p.set_defaults(fn=cmd_add_nodes)
    p = sub.add_parser("upgrade-agents")
    p.add_argument("--tag", help="release tag (default: latest on the feed)")
    p.set_defaults(fn=cmd_upgrade_agents)
    p = sub.add_parser("rebuild")
    p.add_argument("--publish", action="store_true", help="build and publish a release first")
    p.add_argument("--ui", action="store_true", help="with --publish: rebuild the console too")
    p.add_argument("--no-reset", action="store_true", help="skip the checkpoint restore")
    p.set_defaults(fn=cmd_rebuild)
    for name, fn in (("run", cmd_run), ("sudo", lambda a: cmd_run(a, sudo=True))):
        p = sub.add_parser(name)
        p.add_argument("vm")
        p.add_argument("command", nargs=argparse.REMAINDER)
        p.set_defaults(fn=fn)
    args = ap.parse_args()
    if getattr(args, "command", None) and args.command[:1] == ["--"]:
        args.command = args.command[1:]
    args.fn(args)


if __name__ == "__main__":
    main()
