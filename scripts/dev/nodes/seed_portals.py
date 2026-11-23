#!/usr/bin/env python3
"""Deploys the lab portals onto the disposable Docker nodes, through Forge.

    python3 scripts/dev/nodes/seed_portals.py

What it does (safe to run again; existing objects are reused):

  1. PostgreSQL from the App Store on dev-node-3, listening on all addresses.
  2. A `portals` role and database on it (with `docker exec`: running SQL
     through Forge needs the exec WebSocket, which this script does not speak).
  3. Project `portals` with environment `dev` in cluster `dev`.
  4. Services py-portal (Python, 8101) and net-portal (.NET 10, 8103), active
     on dev-node-1 and dev-node-2. Forge installs the runtimes.
  5. The portal sources uploaded into each service's folder with the
     Files API, a log ingest key per service, then a deploy.
  6. Ingress: Nginx on dev-node-3, round robin to node-1/2, published on
     this host.

FORGE_ROTATE_LOG_KEYS=1 replaces the services' log ingest keys (use it when
the portals log `401` to Forge).

Settings come from the environment: FORGE_API (default
http://127.0.0.1:55130), FORGE_LOGIN (admin), FORGE_PASSWORD_FILE
(~/.local/share/forge-dev/admin-password.txt), FORGE_NODE_URL (what nodes
use to reach Forge, default http://172.17.0.1:55130). The database password
is generated once and kept in scripts/dev/nodes/.portals-db-password.
"""
from __future__ import annotations

import base64
import os
import secrets
import shlex
import subprocess
import sys
import time
import urllib.parse
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(REPO / "scripts" / "lab"))
import seed  # noqa: E402  (lab API client: login challenge, cookies, CSRF)

seed.BASE = os.environ.get("FORGE_API", "http://127.0.0.1:55130").rstrip("/")
LOGIN = os.environ.get("FORGE_LOGIN", "admin")
PASSWORD_FILE = Path(os.environ.get("FORGE_PASSWORD_FILE", Path.home() / ".local/share/forge-dev/admin-password.txt"))
NODE_URL = os.environ.get("FORGE_NODE_URL", "http://172.17.0.1:55130")

CLUSTER, PROJECT, ENV = "dev", "portals", "dev"
APP_NODES = ["dev-node-1", "dev-node-2"]
DB_NODE, DB_CONTAINER, DB_HOST = "dev-node-3", "forge-dev-nodes-node-3-1", "172.30.0.13"
# Ingress: an Nginx edge on dev-node-3, round-robin to the portals on node-1/2.
EDGE_NODE, EDGE_IP = "dev-node-3", "172.30.0.13"
# nip.io resolves <anything>.<ip>.nip.io to <ip>: the Tailscale address of this host,
# where a small proxy forwards EDGE_PUBLISH to the edge's port 80.
DOMAIN_SUFFIX = os.environ.get("FORGE_DEV_DOMAIN", "127.0.0.1.nip.io")
EDGE_PUBLISH = os.environ.get("FORGE_DEV_EDGE_PUBLISH", "127.0.0.1:8088")
DOMAINS = {"py-portal": "py", "net-portal": "net"}
KEY_NAME = "dev-nodes"  # log ingest key the script manages
PORTALS = HERE.parent.parent / "lab" / "portals"
SERVICES = [
    # name, port, files to upload (source -> path in the app folder), command, dependencies
    ("py-portal", 8101, {"portal.py": "portal.py", "requirements.txt": "requirements.txt", "run-python.sh": "run-python.sh"},
     "sh {dir}/run-python.sh", [{"package": "python", "version": "system"}]),
    ("net-portal", 8103, {"dotnet/portal.cs": "portal.cs"},
     "dotnet run {dir}/portal.cs", [{"package": "dotnet", "version": "10.0", "params": {"flavor": "sdk"}}]),
]


def step(msg: str) -> None:
    print(f"  - {msg}", flush=True)


def wait(what: str, fn, timeout: int = 900, every: int = 5):
    end = time.time() + timeout
    while time.time() < end:
        r = fn()
        if r:
            return r
        time.sleep(every)
    raise SystemExit(f"timed out waiting for {what}")


def docker(*args: str, stdin: str | None = None) -> str:
    """Runs docker; through `sg docker` when this login session lacks the group."""
    cmd = ["docker", *args]
    if not _in_docker_group():
        cmd = ["sg", "docker", "-c", shlex.join(cmd)]
    return subprocess.run(cmd, input=stdin, check=True, capture_output=True, text=True).stdout.strip()


def psql(sql: str) -> str:
    """Runs SQL as postgres on the database node (SQL on stdin: no shell quoting)."""
    return docker("exec", "-i", DB_CONTAINER, "su", "postgres", "-c",
                  "/usr/lib/postgresql/17/bin/psql -qtAX -v ON_ERROR_STOP=1", stdin=sql)


def _in_docker_group() -> bool:
    return any(g == "docker" for g in subprocess.run(["id", "-nG"], capture_output=True, text=True).stdout.split())


class Seeder:
    def __init__(self) -> None:
        self.api = seed.Api(LOGIN, PASSWORD_FILE.read_text().strip())
        self.servers = {s["hostname"]: s["id"] for s in self.api.get("/api/v1/servers")}
        missing = [n for n in [*APP_NODES, DB_NODE] if n not in self.servers]
        if missing:
            raise SystemExit(f"nodes not enrolled: {', '.join(missing)} (start scripts/dev/nodes first)")
        self.cluster = next(c for c in self.api.get("/api/v1/clusters") if c["name"] == CLUSTER)

    # ---------------------------------------------------------------- database
    def postgres(self) -> str:
        print("== PostgreSQL on", DB_NODE)
        sid = self.servers[DB_NODE]
        inst = [i for i in self.api.get(f"/api/v1/installations?server_id={sid}") if i["package"] == "postgresql"]
        if not inst or inst[0]["status"] != "installed":
            self.api.post("/api/v1/packages/install",
                          {"package": "postgresql", "version": "17", "server_ids": [sid], "params": {"port": "5432", "bind": "all"}})
            step("installing postgresql 17 from the App Store ...")

            def done():
                for i in self.api.get(f"/api/v1/installations?server_id={sid}"):
                    if i["package"] == "postgresql":
                        if i["status"] == "failed":
                            raise SystemExit(f"postgresql install failed: {i.get('last_error')}")
                        return i["status"] == "installed"
                return False
            wait("postgresql install", done, timeout=1200, every=10)
        step("postgresql installed")

        pw_file = HERE / ".portals-db-password"
        if not pw_file.exists():
            pw_file.write_text(secrets.token_urlsafe(18) + "\n")
            pw_file.chmod(0o600)
        pw = pw_file.read_text().strip()
        sql = (f"DO $$BEGIN IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname='portals') THEN "
               f"CREATE ROLE portals LOGIN PASSWORD '{pw}'; ELSE ALTER ROLE portals PASSWORD '{pw}'; END IF; END$$;")
        psql(sql)
        if psql("SELECT 1 FROM pg_database WHERE datname='portals';") != "1":
            psql("CREATE DATABASE portals OWNER portals;")
        step("role and database `portals` ready")
        return f"postgres://portals:{pw}@{DB_HOST}:5432/portals"

    # ---------------------------------------------------------------- project + env
    def environment(self) -> dict:
        print("== Project and environment")
        projects = self.api.get("/api/v1/projects")
        project = next((p for p in projects if p["name"] == PROJECT), None)
        if not project:
            project = self.api.post("/api/v1/projects", {"name": PROJECT, "description": "Python and .NET test portals"})
        step(f"project {PROJECT}")
        ns = next((n for n in self.api.get(f"/api/v1/environments?cluster_id={self.cluster['id']}") if n["name"] == ENV), None)
        if not ns:
            ns = self.api.post("/api/v1/environments",
                               {"name": ENV, "cluster_id": self.cluster["id"], "project_id": project["id"], "protected": False})
        step(f"environment {ENV} in cluster {CLUSTER}")
        return ns

    # ---------------------------------------------------------------- services
    def services(self, ns: dict, db_url: str) -> None:
        print("== Services")
        existing = {a["name"]: a for a in self.api.get(f"/api/v1/services?environment_id={ns['id']}")}
        for name, port, files, command, deps in SERVICES:
            body = self.service_body(ns, name, port, command.format(dir="."), deps, db_url, log_key=None)
            app = existing.get(name) or self.api.post("/api/v1/services", body)
            step(f"service {name}")
            # Placing the service prepares the nodes: account, app folder, file root.
            self.api.put(f"/api/v1/services/{app['id']}/instances",
                         [{"server_id": self.servers[n], "role": "active"} for n in APP_NODES])
            app_dir = None
            for n in APP_NODES:
                root = wait(f"app folder of {name} on {n}", lambda n=n: self.app_root(n, app["id"]), timeout=600)
                app_dir = root["base"]
                for src, dst in files.items():
                    self.upload(n, root, PORTALS / src, dst)
                step(f"{name}: sources uploaded to {n}:{app_dir}")
            key = self.log_key(app["id"])
            body = self.service_body(ns, name, port, command.format(dir=app_dir), deps, db_url, log_key=key)
            if key is None:  # keep the stored secret: the API keeps it when the value is null
                body["env"] = [e for e in body["env"] if e["key"] != "FORGE_LOG_KEY"] + [
                    {"key": "FORGE_LOG_KEY", "value": None, "secret": True}]
            self.api.put(f"/api/v1/services/{app['id']}", body)
            self.api.post(f"/api/v1/services/{app['id']}/deploy")
            step(f"{name}: deploying to {', '.join(APP_NODES)} (runtimes are installed first)")
            existing[name] = app

        def all_up():
            ok = True
            for name, *_ in SERVICES:
                for i in self.api.get(f"/api/v1/services/{existing[name]['id']}")["instances"]:
                    if i["status"] == "failed":
                        raise SystemExit(f"{name} failed on a node: {i.get('last_error')}")
                    ok &= i["status"] == "deployed"
            return ok
        wait("deploys", all_up, timeout=1800, every=10)
        step("all services deployed")

    # ---------------------------------------------------------------- ingress
    def install(self, node: str, package: str, version: str) -> None:
        sid = self.servers[node]
        if any(i["package"] == package and i["status"] == "installed" for i in self.api.get(f"/api/v1/installations?server_id={sid}")):
            return
        self.api.post("/api/v1/packages/install", {"package": package, "version": version, "server_ids": [sid], "params": {}})
        step(f"installing {package} on {node} ...")

        def done():
            for i in self.api.get(f"/api/v1/installations?server_id={sid}"):
                if i["package"] == package:
                    if i["status"] == "failed":
                        raise SystemExit(f"{package} install failed: {i.get('last_error')}")
                    return i["status"] == "installed"
            return False
        wait(f"{package} on {node}", done, timeout=900, every=5)

    def ingress(self) -> None:
        print("== Ingress (Nginx edge on", EDGE_NODE + ")")
        self.install(EDGE_NODE, "nginx", "stable")
        step("nginx installed")
        cid = self.cluster["id"]
        nginx = {"worker_connections": 1024, "keepalive_timeout_s": 65, "proxy_read_timeout_s": 60, "client_max_body_mb": 25,
                 "gzip": True, "server_tokens": False, "hsts": False, "real_ip_from": [], "rate_limit": 0,
                 "access_log": "combined", "custom": ""}
        self.api.put(f"/api/v1/clusters/{cid}/profile", {"ingress": {
            "mode": "standalone", "failover": "dns", "vrrp": None, "nginx": nginx,
            "edges": [{"server_id": self.servers[EDGE_NODE], "role": "primary", "public_ip": EDGE_IP}]}})
        r = self.api.post(f"/api/v1/clusters/{cid}/profile/apply")
        bad = {k: v for k, v in r["edges"].items() if v["status"] != "applied"}
        if bad:
            raise SystemExit(f"ingress profile apply failed: {bad}")
        step(f"cluster ingress: standalone edge {EDGE_NODE}")
        ns = next(n for n in self.api.get(f"/api/v1/environments?cluster_id={cid}") if n["name"] == ENV)
        apps = {a["name"]: a for a in self.api.get(f"/api/v1/services?environment_id={ns['id']}")}
        sites = {x["name"]: x for x in self.api.get("/api/v1/sites")}
        for name, port, *_ in SERVICES:
            domain = f"{DOMAINS[name]}.{DOMAIN_SUFFIX}"
            body = {"name": name, "domains": [domain], "upstreams": [], "cluster_id": cid,
                    "targets": [{"kind": "app", "service_id": apps[name]["id"], "port": port}],
                    "balance": "round_robin", "tls": "off", "websockets": True, "max_body_mb": 25, "rate_limit": 0, "custom": ""}
            site = self.api.put(f"/api/v1/sites/{sites[name]['id']}", body) if name in sites else self.api.post("/api/v1/sites", body)
            r = self.api.post(f"/api/v1/sites/{site['id']}/apply")
            bad = [e for e in r["edges"] if e["result"]["status"] != "succeeded"]
            if bad:
                raise SystemExit(f"{domain}: apply failed: {bad[0]['result'].get('error')}")
            step(f"endpoint {domain} -> {name}:{port} (round robin over {', '.join(APP_NODES)})")
        self.publish_edge()

    def publish_edge(self) -> None:
        """Forwards EDGE_PUBLISH on this host to the edge's port 80 (a socat container)."""
        name = "forge-dev-edge-proxy"
        bind, port = EDGE_PUBLISH.rsplit(":", 1)
        running = docker("ps", "-q", "--filter", f"name=^{name}$")
        if not running:
            docker("rm", "-f", name) if docker("ps", "-aq", "--filter", f"name=^{name}$") else None
            docker("run", "-d", "--name", name, "--restart", "unless-stopped", "--network", "host", "alpine/socat:1.8.0.3",
                   f"TCP-LISTEN:{port},bind={bind},fork,reuseaddr", f"TCP:{EDGE_IP}:80")
        step(f"edge published on {EDGE_PUBLISH}")

    @staticmethod
    def service_body(ns, name, port, command, deps, db_url, log_key):
        env = [
            {"key": "PORT", "value": str(port), "secret": False},
            {"key": "FORGE_URL", "value": NODE_URL, "secret": False},
            {"key": "DATABASE_URL", "value": db_url, "secret": True},
            {"key": "DOTNET_CLI_TELEMETRY_OPTOUT", "value": "1", "secret": False},
            {"key": "DOTNET_NOLOGO", "value": "1", "secret": False},
        ]
        if log_key:
            env.append({"key": "FORGE_LOG_KEY", "value": log_key, "secret": True})
        return {"environment_id": ns["id"], "name": name, "runtime": "systemd", "command": command,
                "working_dir": "", "image": "", "ports": [], "volumes": [],
                "env_files": [], "restart": True, "dependencies": deps, "env": env}

    def log_key(self, service_id: str) -> str | None:
        """A new ingest key the first time; None when the service has one (the
        stored secret is kept). FORGE_ROTATE_LOG_KEYS=1 revokes this script's
        keys and issues new ones, for when the stored secret was lost."""
        keys = self.api.get(f"/api/v1/services/{service_id}/log-keys")
        if os.environ.get("FORGE_ROTATE_LOG_KEYS") == "1":
            for k in keys:
                if k["name"] == KEY_NAME:
                    self.api.call("DELETE", f"/api/v1/log-keys/{k['id']}")
            keys = [k for k in keys if k["name"] != KEY_NAME]
        if any(k["name"] == KEY_NAME for k in keys):
            return None
        return self.api.post(f"/api/v1/services/{service_id}/log-keys", {"name": KEY_NAME})["token"]

    def app_root(self, node: str, service_id: str):
        sid = self.servers[node]
        roots = self.api.get(f"/api/v1/servers/{sid}/files/roots").get("roots", [])
        root = next((r for r in roots if r.get("service_id") == service_id), None)
        if not root:
            return None
        # The root is registered before the `prepare` job has created the folder.
        st = self.api.call("GET", f"/api/v1/servers/{sid}/files/stat?root={root['id']}&path=/",
                           ok=(200, 404))
        return root if st and st.get("entry") else None

    def upload(self, node: str, root: dict, src: Path, dst: str) -> None:
        sid = self.servers[node]
        content = base64.b64encode(src.read_bytes()).decode()
        dst = f"/{dst}"  # Files API paths are relative to the root and start with `/`
        listing = self.api.call("GET", f"/api/v1/servers/{sid}/files/stat?root={root['id']}&path={urllib.parse.quote(dst)}", ok=(200, 404))
        body = {"root": root["id"], "path": dst, "content_b64": content}
        if listing and listing.get("version"):
            body["version"] = listing["version"]
        self.api.post(f"/api/v1/servers/{sid}/files/save", body)


def main() -> None:
    s = Seeder()
    db_url = s.postgres()
    ns = s.environment()
    s.services(ns, db_url)
    s.ingress()
    print("\nThrough the ingress (round robin):")
    for name, *_ in SERVICES:
        print(f"  {name:<11} http://{DOMAINS[name]}.{DOMAIN_SUFFIX}:{EDGE_PUBLISH.rsplit(':', 1)[1]}/")
    print("\nEach portal also answers on each app node:")
    for name, port, *_ in SERVICES:
        for n, ip in zip(APP_NODES, ["172.30.0.11", "172.30.0.12"]):
            print(f"  {name:<11} {n}: http://{ip}:{port}/")


if __name__ == "__main__":
    main()
