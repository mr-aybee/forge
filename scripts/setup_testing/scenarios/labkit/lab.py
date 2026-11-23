"""Shared helpers for the deployment scenarios (10–17).

Stdlib only (Python 3.10+), so a scenario runs the same from the Windows
host or a Linux box, wherever the server is. Forge is driven through the
`forge` CLI (its signed-in context): typed `forge deploy …` commands where
the scenario is about the CLI, `forge api request` for everything else, and
`forge nodes exec` to look at nodes (no SSH needed).

Before a run: `forge auth login` once in the context the scripts use
(`--context`, default the current one). On the lab: plain HTTP needs
FORGE_ALLOW_HTTP=1 (set here when `--server` is http).
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCENARIOS = HERE.parent
SETUP_TESTING = SCENARIOS.parent
# Where the setup script writes what later scenarios need (ids, URLs, digests).
STATE_FILE = SETUP_TESTING / ".lab" / "deploy-lab.json"
# Written by labkit/bootstrap.py: the server URL and the scripts' API key.
CLIENT_FILE = SETUP_TESTING / ".lab" / "lab-client.json"
TOKEN_FILE = SETUP_TESTING / ".lab" / "forge-admin.token"

GREEN, RED, YELLOW, DIM, RESET = ("\033[32m", "\033[31m", "\033[33m", "\033[2m", "\033[0m") if sys.stdout.isatty() else ("",) * 5


class LabError(RuntimeError):
    """A step could not run (not a failed check)."""


@dataclass
class Result:
    code: int
    result: object = None
    error: dict | None = None
    stderr: str = ""

    @property
    def ok(self) -> bool:
        return self.code == 0

    @property
    def message(self) -> str:
        if self.error:
            return str(self.error.get("message", self.error)) + (f" {self.stderr.strip()}" if "deployment" in self.stderr else "")
        return self.stderr.strip()


@dataclass
class Check:
    scenario: str
    name: str
    passed: bool
    detail: str = ""


def parser(description: str) -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=description)
    p.add_argument("--forge", default=os.environ.get("FORGE_BIN", "forge"), help="the forge CLI (default: on PATH)")
    p.add_argument("--context", default=os.environ.get("FORGE_CONTEXT"), help="forge context to use")
    client = json.loads(CLIENT_FILE.read_text()) if CLIENT_FILE.exists() else {}
    p.add_argument("--server", default=os.environ.get("FORGE_SERVER") or client.get("server"),
                   help="Forge server URL (default: .lab/lab-client.json; in WSL http://127.0.0.1:18080 via labkit/forward.py)")
    p.add_argument("--edge", default=os.environ.get("FORGE_LAB_EDGE") or client.get("edge", "10.50.0.10"),
                   help="edge (nginx) host[:port] for traffic (in WSL 127.0.0.1:18081)")
    p.add_argument("--docker-node", default="lab-c", help="node with Docker")
    p.add_argument("--nodes", default="lab-b,lab-c", help="worker nodes, comma-separated")
    p.add_argument("--only", action="append", default=[], help="run only these checks (e.g. D03); repeatable")
    p.add_argument("--keep", action="store_true", help="leave services and endpoints in place")
    p.add_argument("--reset", action="store_true", help="restore checkpoint deploy-ready first (Hyper-V host only)")
    p.add_argument("--timeout", type=int, default=20, help="minutes to wait for one rollout")
    return p


class Lab:
    """One run of one scenario: CLI access, shared state, checks."""

    def __init__(self, args: argparse.Environment, scenario: str):
        self.args = args
        self.scenario = scenario
        self.checks: list[Check] = []
        self.work = Path(tempfile.mkdtemp(prefix=f"forge-{scenario}-"))
        self.forge_bin = shutil.which(args.forge) or args.forge
        self.state = json.loads(STATE_FILE.read_text()) if STATE_FILE.exists() else {}
        os.environ.setdefault("FORGE_ALLOW_HTTP", "1")
        # Without a context: the server and the bootstrap's API key.
        if not args.context and args.server:
            os.environ["FORGE_SERVER"] = args.server
            if TOKEN_FILE.exists() and not os.environ.get("FORGE_TOKEN"):
                os.environ["FORGE_TOKEN"] = TOKEN_FILE.read_text().strip()
        if getattr(args, "reset", False):
            reset_checkpoint()

    # ---- running forge ------------------------------------------------------

    def forge_as(self, token: str, *argv: str, timeout: int = 900) -> Result:
        """Runs forge as another principal (a token, never stored by the CLI)."""
        return self.forge(*argv, timeout=timeout, token=token)

    def admin_session(self):
        """The lab admin signed in with the password (`.lab/forge-admin.env`).

        Forge creates service accounts and their tokens only for a signed-in
        person, never for an API key (a leaked key must not mint identities).
        """
        from labkit.session import Session

        env = {}
        f = SETUP_TESTING / ".lab" / "forge-admin.env"
        for line in f.read_text().splitlines() if f.exists() else []:
            if "=" in line and not line.startswith("#"):
                k, v = line.split("=", 1)
                env[k.strip()] = v.strip()
        if not env.get("FORGE_ADMIN_PASSWORD"):
            raise LabError(f"no admin login in {f}")
        return Session(self.args.server, env.get("FORGE_ADMIN_USER", "admin"), env["FORGE_ADMIN_PASSWORD"])

    def service_account(self, name: str, description: str, token_body: dict) -> dict:
        """Creates service account `svc-<name>` (if new) and issues a token; returns it with `secret`."""
        s = self.admin_session()
        s.call("POST", "/api/v1/service-accounts", {"name": name, "description": description})
        status, accounts = s.call("GET", "/api/v1/service-accounts")
        acc = next((x for x in accounts or [] if x.get("name") in (name, f"svc-{name}")), None) if status == 200 else None
        if not acc:
            raise LabError(f"service account {name} was not created ({status})")
        status, tok = s.call("POST", f"/api/v1/service-accounts/{acc['id']}/tokens", token_body)
        if status != 201:
            raise LabError(f"token for {name}: {status} {tok}")
        return tok  # type: ignore[return-value]

    def approver_token(self) -> str:
        f = Path(self.state.get("approver_token_file", ""))
        if not f.is_file():
            # Written on Windows, read in WSL (or the other way): same file in .lab/.
            f = SETUP_TESTING / ".lab" / "deploy-approver.token"
        if not f.is_file():
            raise LabError("no approver token: run 10-deploy-setup.py first")
        return f.read_text().strip()

    def forge(self, *argv: str, stdin: str | None = None, timeout: int = 900, token: str | None = None) -> Result:
        cmd = [self.forge_bin, "-o", "json", "--non-interactive"]
        if self.args.context:
            cmd += ["--context", self.args.context]
        cmd += list(argv)
        env = dict(os.environ)
        if token:
            env["FORGE_TOKEN"] = token
        env.pop("FORGE_CONTEXT", None) if not self.args.context else None
        try:
            # Always a stdin that ends: `nodes exec` forwards a piped stdin and
            # waits for its end, so an inherited open pipe would hang it.
            p = subprocess.run(cmd, input=stdin if stdin is not None else "", capture_output=True, text=True,
                               timeout=timeout, env=env)
        except FileNotFoundError as e:
            raise LabError(f"forge CLI not found ({self.forge_bin}); pass --forge") from e
        except subprocess.TimeoutExpired as e:
            return Result(124, stderr=f"timed out after {timeout}s: {' '.join(argv)}\n{e.stderr or ''}")
        doc: dict = {}
        if p.stdout.strip():
            try:
                doc = json.loads(p.stdout)
            except json.JSONDecodeError:
                doc = {"result": p.stdout}
        return Result(p.returncode, doc.get("result"), doc.get("error"), p.stderr)

    def must(self, *argv: str, stdin: str | None = None, timeout: int = 900) -> object:
        r = self.forge(*argv, stdin=stdin, timeout=timeout)
        if not r.ok:
            raise LabError(f"forge {' '.join(argv)} failed ({r.code}): {r.message}")
        return r.result

    def api(self, method: str, path: str, body: object | None = None) -> Result:
        argv = ["api", "request", method, path, "--yes"]
        if body is not None:
            f = self.work / f"body-{time.time_ns()}.json"
            f.write_text(json.dumps(body))
            argv += ["--body", f"@{f}"]
        return self.forge(*argv)

    def api_ok(self, method: str, path: str, body: object | None = None) -> object:
        r = self.api(method, path, body)
        if not r.ok:
            raise LabError(f"{method} {path} failed ({r.code}): {r.message}")
        return r.result

    # ---- lab objects --------------------------------------------------------

    def nodes(self) -> dict[str, dict]:
        """Node hostname → node."""
        return {n["hostname"]: n for n in (self.must("nodes", "list") or [])}

    def node_id(self, name: str) -> str:
        n = self.nodes().get(name)
        if not n:
            raise LabError(f"no node {name}")
        return n["id"]

    def cluster_id(self, name: str = "lab") -> str:
        for c in self.must("clusters", "list") or []:
            if c["name"] == name:
                return c["id"]
        raise LabError(f"no cluster {name}: create it first (10-deploy-setup)")

    def environment(self, name: str) -> str:
        """The environment `name`, created in cluster `lab` when missing."""
        for n in self.must("environments", "list", "--all") or []:
            if n["name"] == name:
                return n["id"]
        # Environments belong to a project: one `lab` for all scenarios.
        if not any(g.get("name") == "lab" for g in self.must("projects", "list", "--all") or []):
            self.must("projects", "create", "lab", "--description", "Lab scenarios")
        self.must("environments", "create", name, "--cluster", "lab", "--project", "lab")
        return self.environment(name)

    def service(self, name: str, environment: str) -> dict | None:
        ns = self.environment(environment)
        r = self.api("GET", f"/api/v1/services?environment_id={ns}")
        for a in (r.result or []) if r.ok else []:
            if a["name"] == name:
                return a
        return None

    def node_sh(self, node: str, command: str, stdin: str | None = None, timeout: int = 300) -> Result:
        """Runs a shell command line on a node as root (`forge nodes exec`)."""
        r = self.forge("nodes", "exec", node, "--login", "root", "--shell", "--propagate-exit-code", "--", command, stdin=stdin, timeout=timeout)
        # `result` is {stdout, stderr, exit_code}: the scripts want the output text.
        if isinstance(r.result, dict) and "stdout" in r.result:
            out, err, code = r.result.get("stdout", ""), r.result.get("stderr", ""), r.result.get("exit_code", r.code)
            return Result(0 if code == 0 else (code or 1), out, None if code == 0 else {"message": (err or out).strip()}, err)
        return r

    # ---- deployments --------------------------------------------------------

    def doc_file(self, doc: dict, name: str = "forge.deploy.yaml") -> Path:
        """Writes a deployment document. JSON is valid YAML, so no YAML library is needed."""
        f = self.work / name
        f.write_text(json.dumps(doc, indent=2))
        return f

    def apply(self, doc: dict, wait: bool = True, environment: str | None = None) -> Result:
        f = self.doc_file(doc)
        argv = ["deploy", "apply", "-f", str(f)]
        if environment:
            argv += ["--environment", environment]
        if wait:
            argv += ["--wait", "--timeout", str(self.args.timeout)]
        r = self.forge(*argv, timeout=self.args.timeout * 60 + 60)
        if not r.ok:
            r.stderr = (r.stderr + " " + self.why(doc)).strip()
        return r

    def why(self, doc: dict) -> str:
        """The last deployment's error and events of a document's service (for a failed apply)."""
        meta = doc.get("metadata", {})
        h = self.forge("deploy", "history", meta.get("name", ""), "-n", meta.get("environment", ""), "--limit", "1")
        rows = h.result if h.ok and isinstance(h.result, list) else []
        if not rows:
            return h.message
        d = self.forge("deploy", "status", rows[0]["id"])
        events = (d.result or {}).get("events", []) if d.ok and isinstance(d.result, dict) else []
        tail = " | ".join(e.get("message", "") for e in events[-5:])
        return f"deployment {rows[0]['state']}: {rows[0].get('error') or ''} [{tail}]"

    def plan(self, doc: dict, environment: str | None = None) -> Result:
        f = self.doc_file(doc)
        argv = ["deploy", "plan", "-f", str(f)]
        if environment:
            argv += ["--environment", environment]
        return self.forge(*argv)

    def deployment(self, dep_id: str) -> dict:
        return self.must("deploy", "status", dep_id)  # type: ignore[return-value]

    def last_deployment(self, service: str, environment: str) -> dict | None:
        r = self.forge("deploy", "history", service, "-n", environment, "--limit", "1")
        rows = r.result if r.ok else []
        return rows[0] if rows else None

    def wait_state(self, dep_id: str, states: set[str], timeout: float = 600) -> dict:
        """Polls a deployment until its state is one of `states`."""
        end = time.time() + timeout
        while True:
            d = self.deployment(dep_id)
            if d["state"] in states:
                return d
            if time.time() > end:
                raise LabError(f"deployment {dep_id} is still {d['state']} after {timeout:.0f}s")
            time.sleep(2)

    def slots(self, service: str, environment: str) -> dict:
        return self.must("deploy", "slots", service, "-n", environment)  # type: ignore[return-value]

    def endpoint(self, name: str, domain: str, service_id: str, port: int, edges: list[str]) -> None:
        """Creates (or updates) and applies an endpoint that routes to a service."""
        body = {
            "name": name, "domains": [domain], "upstreams": [], "balance": "round_robin", "tls": "off",
            "edges": edges, "targets": [{"kind": "app", "service_id": service_id, "port": port}],
        }
        f = self.work / f"{name}.site.json"
        f.write_text(json.dumps(body))
        if self.forge("sites", "create", "--file", str(f)).code != 0:
            self.must("sites", "update", name, "--file", str(f), "--yes")
        self.must("sites", "apply", name, "--yes", timeout=300)

    def drop_endpoint(self, name: str) -> None:
        self.forge("sites", "remove", name, "--yes")
        self.forge("sites", "delete", name, "--yes")

    def drop_stack(self, name: str) -> None:
        """Takes a compose stack down (volumes too) and deletes it (best effort)."""
        self.forge("stacks", "down", name, "--volumes", "--wait", "--yes", timeout=600)
        self.forge("stacks", "delete", name, "--yes")

    def drop_service(self, name: str, environment: str) -> None:
        """Removes a service from its nodes and deletes it (best effort)."""
        a = self.service(name, environment)
        if not a:
            return
        for i in a.get("instances", []) or self.api_ok("GET", f"/api/v1/services/{a['id']}").get("instances", []):  # type: ignore[union-attr]
            self.api("POST", f"/api/v1/services/{a['id']}/instances/{i['server_id']}/actions", {"action": "remove"})
        end = time.time() + 180
        while time.time() < end:
            if self.api("DELETE", f"/api/v1/services/{a['id']}").ok:
                return
            time.sleep(3)

    # ---- services, packages, stacks (the console's flows) --------------------

    def install_package(self, package: str, version: str, nodes: list[str], params: dict | None = None,
                        timeout: int = 1800) -> bool:
        """Installs an App Store package on nodes (by hostname) unless it is there; waits."""
        ids = [self.node_id(n) for n in nodes]
        have = self.forge("packages", "installed")
        rows = have.result or [] if have.ok else []
        missing = [i for i in ids if not any(r.get("package") == package and r.get("server_id") == i
                                               and r.get("status") == "installed" for r in rows)]
        if not missing:
            return True
        f = self.work / f"pkg-{package}.json"
        f.write_text(json.dumps({"package": package, "version": version, "params": params or {}, "server_ids": missing}))
        r = self.forge("packages", "install", "--file", str(f), "--wait", timeout=timeout)
        if not r.ok:
            r = self.forge("packages", "install", "--file", str(f), timeout=timeout)
        end = time.time() + timeout
        while time.time() < end:
            rows = self.forge("packages", "installed").result or []
            states = [r.get("status") for r in rows if r.get("package") == package and r.get("server_id") in missing]
            if states and all(s == "installed" for s in states):
                return True
            if any(s == "failed" for s in states):
                return False
            time.sleep(5)
        return False

    def create_app(self, environment: str, body: dict) -> dict:
        """Creates a service (`forge services create --file`) and returns it; replaces one left over."""
        self.drop_service(body["name"], environment)
        body = dict(body, environment_id=self.environment(environment))
        f = self.work / f"app-{body['name']}.json"
        f.write_text(json.dumps(body))
        return self.must("services", "create", "--file", str(f))  # type: ignore[return-value]

    def place_and_deploy(self, app: str, environment: str, nodes: list[str], timeout: int = 1800) -> Result:
        self.must("services", "place", app, "--environment", environment, *sum((["--node", n] for n in nodes), []), "--yes")
        return self.forge("services", "deploy", app, "--environment", environment, "--wait", "--yes", timeout=timeout)

    def upload_and_extract(self, app: str, environment: str, archive: Path, dest: str) -> Result:
        self.forge("services", "files", app, "--environment", environment, "upload", str(archive), f"/{archive.name}", "--overwrite")
        return self.forge("services", "files", app, "--environment", environment, "extract", f"/{archive.name}", dest, "--replace")

    def site_zip(self, name: str, html: str, extra: dict[str, str] | None = None) -> Path:
        """A zip with `index.html` (and more files) at its top."""
        import zipfile

        f = self.work / f"{name}.zip"
        with zipfile.ZipFile(f, "w") as z:
            z.writestr("index.html", html)
            for k, v in (extra or {}).items():
                z.writestr(k, v)
        return f

    def http(self, host: str, path: str = "/", edge: str | None = None, timeout: float = 5) -> tuple[int, str]:
        """GET through the edge with a Host header: (status, body)."""
        req = urllib.request.Request(f"http://{edge or self.args.edge}{path}", headers={"Host": host})
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                return r.status, r.read(4096).decode(errors="replace")
        except urllib.error.HTTPError as e:
            return e.code, ""
        except OSError as e:
            return 0, str(e)

    def server_ok(self) -> bool:
        """The Forge server answers its health check."""
        try:
            with urllib.request.urlopen(f"{self.args.server.rstrip('/')}/api/health", timeout=5) as r:
                return r.status == 200
        except OSError:
            return False

    def site(self, body: dict) -> Result:
        """Creates (or updates) and applies an endpoint from a full body."""
        f = self.work / f"{body['name']}.site.json"
        f.write_text(json.dumps(body))
        if self.forge("sites", "create", "--file", str(f)).code != 0:
            self.must("sites", "update", body["name"], "--file", str(f), "--yes")
        return self.forge("sites", "apply", body["name"], "--yes", timeout=300)

    # ---- checks -------------------------------------------------------------

    def wanted(self, check_id: str) -> bool:
        return not self.args.only or check_id in self.args.only

    def check(self, check_id: str, name: str, passed: bool, detail: str = "") -> bool:
        c = Check(check_id, name, bool(passed), detail)
        self.checks.append(c)
        mark = f"{GREEN}PASS{RESET}" if c.passed else f"{RED}FAIL{RESET}"
        print(f"  [{mark}] {check_id} {name}" + (f"  {DIM}{detail}{RESET}" if detail else ""), flush=True)
        return c.passed

    def step(self, text: str) -> None:
        print(f"{YELLOW}▸{RESET} {text}", flush=True)

    def finish(self) -> int:
        failed = [c for c in self.checks if not c.passed]
        print()
        print(f"{self.scenario}: {len(self.checks) - len(failed)} passed, {len(failed)} failed")
        for c in failed:
            print(f"  {RED}✗{RESET} {c.scenario} {c.name}: {c.detail}")
        return 1 if failed or not self.checks else 0

    def save_state(self, **values: object) -> None:
        self.state.update(values)
        STATE_FILE.parent.mkdir(parents=True, exist_ok=True)
        STATE_FILE.write_text(json.dumps(self.state, indent=2))


def run(main) -> None:
    """Runs a scenario's `main(lab)`; a step that cannot run counts as a failure."""
    try:
        sys.exit(main())
    except LabError as e:
        print(f"{RED}cannot continue:{RESET} {e}")
        sys.exit(2)
    except KeyboardInterrupt:
        sys.exit(130)


# ---- traffic -----------------------------------------------------------------


@dataclass
class Traffic:
    """Requests through the edge in a background thread, counting answers."""

    edge: str
    host: str
    path: str = "/"
    interval: float = 0.05
    statuses: dict[int, int] = field(default_factory=dict)
    bodies: dict[str, int] = field(default_factory=dict)
    failures: int = 0
    total: int = 0
    _stop: threading.Event = field(default_factory=threading.Event)
    _thread: threading.Thread | None = None

    def _one(self) -> None:
        req = urllib.request.Request(f"http://{self.edge}{self.path}", headers={"Host": self.host})
        try:
            with urllib.request.urlopen(req, timeout=5) as r:
                status, body = r.status, r.read(2048).decode(errors="replace")
        except urllib.error.HTTPError as e:
            status, body = e.code, ""
        except OSError:
            status, body = 0, ""
        self.total += 1
        self.statuses[status] = self.statuses.get(status, 0) + 1
        if status == 0 or status >= 500:
            self.failures += 1
        key = body.strip().splitlines()[0] if body.strip() else ""
        self.bodies[key] = self.bodies.get(key, 0) + 1

    def start(self) -> "Traffic":
        def loop() -> None:
            while not self._stop.is_set():
                self._one()
                time.sleep(self.interval)

        self._thread = threading.Thread(target=loop, daemon=True)
        self._thread.start()
        return self

    def stop(self) -> "Traffic":
        self._stop.set()
        if self._thread:
            self._thread.join(timeout=10)
        return self

    def sample(self, n: int) -> dict[str, int]:
        """`n` requests now; first line of each body → count."""
        seen: dict[str, int] = {}
        for _ in range(n):
            before = dict(self.bodies)
            self._one()
            for k, v in self.bodies.items():
                if v != before.get(k, 0):
                    seen[k] = seen.get(k, 0) + 1
        return seen


def reset_checkpoint(name: str = "deploy-ready") -> None:
    """Restores the lab VMs to `name` (runs vm_lab.py on the Hyper-V host)."""
    vm_lab = SETUP_TESTING / "vm_lab.py"
    print(f"{YELLOW}▸{RESET} restoring checkpoint {name}")
    r = subprocess.run([sys.executable, str(vm_lab), "reset", "--checkpoint", name])
    if r.returncode != 0:
        raise LabError("vm_lab.py reset failed (run on the Hyper-V host as an administrator)")


def image_doc(name: str, environment: str, image: str, *, ports: list[str], strategy: dict | None = None,
              entrypoint: str = "", health: dict | None = None, placement: dict | None = None,
              traffic: dict | None = None, rollback: dict | None = None, description: str = "") -> dict:
    """A `forge/v1 Deployment` for a container."""
    spec: dict = {"runtime": "container", "artifact": {"image": image}, "template": {"ports": ports}}
    if entrypoint:
        spec["template"]["entrypoint"] = entrypoint
    for k, v in (("strategy", strategy), ("placement", placement), ("traffic", traffic), ("rollback", rollback)):
        if v is not None:
            spec[k] = v
    spec["health"] = health or {"startupTimeout": "3m", "stabilizationPeriod": "10s"}
    return {"apiVersion": "forge/v1", "kind": "Deployment",
            "metadata": {"name": name, "environment": environment, "description": description}, "spec": spec}
