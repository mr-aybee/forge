#!/usr/bin/env python3
"""Seeds the WSL lab with clusters, enrolled nodes, environments, services, a
network plan per cluster and a vault credential, so every console view has
something to show. Safe to re-run: existing items are reused.

    wsl -d Ubuntu-24.04 -- python3 scripts/lab/seed.py

Layout:
    cluster "lab" (LA): node-1, node-2
        environment payments: api (system)
        environment shop:     web (container)
    cluster "uat" (UA): node-3
        environment payments: api (service)
"""
import hashlib
import http.cookiejar
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
BASE = "http://localhost:8090"


def creds():
    env = {}
    with open(os.path.join(HERE, ".lab", "credentials.env")) as f:
        for line in f:
            if "=" in line:
                k, v = line.strip().split("=", 1)
                env[k] = v
    return env


def login_challenge(login):
    """Solves the server's login challenge (proof of work, D-064), if any."""
    url = f"{BASE}/api/v1/auth/challenge?login={urllib.parse.quote(login)}"
    try:
        with urllib.request.urlopen(url, timeout=30) as r:
            c = json.loads(r.read())
    except (urllib.error.URLError, ValueError):
        return {}
    if not c.get("required"):
        return {}
    salt, bits, n = bytes.fromhex(c["salt"]), int(c["difficulty"]), 0
    while int.from_bytes(hashlib.sha256(salt + str(n).encode()).digest(), "big") >> (256 - bits):
        n += 1
    return {"challenge_id": c["id"], "nonce": str(n)}


class Api:
    def __init__(self, email, password):
        self.jar = http.cookiejar.CookieJar()
        self.op = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(self.jar))
        self.csrf = None
        r = self.call("POST", "/api/v1/auth/login", {"login": email, "password": password, **login_challenge(email)})
        self.csrf = r["csrf_token"]

    def call(self, method, path, body=None, ok=(200, 201, 202, 204)):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(BASE + path, data=data, method=method)
        req.add_header("Content-Type", "application/json")
        if self.csrf:
            req.add_header("X-CSRF-Token", self.csrf)
        try:
            with self.op.open(req, timeout=60) as resp:
                text = resp.read().decode()
                return json.loads(text) if text else None
        except urllib.error.HTTPError as e:
            msg = e.read().decode()
            if e.code in ok:
                return json.loads(msg) if msg else None
            raise SystemExit(f"{method} {path} -> {e.code}: {msg}")

    get = lambda self, p: self.call("GET", p)
    post = lambda self, p, b=None: self.call("POST", p, b or {})
    put = lambda self, p, b: self.call("PUT", p, b)
    patch = lambda self, p, b: self.call("PATCH", p, b)


def step(msg):
    print(f"• {msg}", flush=True)


def main():
    c = creds()
    api = Api(c["ADMIN_EMAIL"], c["ADMIN_PASSWORD"])

    # Clusters (with badges).
    clusters = {x["name"]: x for x in api.get("/api/v1/clusters")}
    for name, abbr, desc in [("lab", "LA", "WSL test nodes"), ("uat", "UA", "Acceptance")]:
        if name not in clusters:
            clusters[name] = api.post("/api/v1/clusters", {"name": name, "description": desc, "abbr": abbr})
            step(f"cluster {name} created")
        else:
            step(f"cluster {name} exists")

    # Enroll node-1..3 with the real installer, as a fresh VM would.
    servers = {s["hostname"]: s for s in api.get("/api/v1/servers")}
    for n in ["node-1", "node-2", "node-3"]:
        if n in servers and servers[n]["status"] != "revoked":
            step(f"{n} already enrolled ({servers[n]['status']})")
            continue
        tok = api.post("/api/v1/enrollment-tokens", {"ttl_minutes": 30})["token"]
        script = (
            'su forgeops -c "curl -fsSL http://forge-cp:8090/install.sh -o /tmp/forge-install.sh" && '
            'echo "$P" | su forgeops -c "sudo -S -p \\"\\" env FORGE_ENROLL_TOKEN=$T sh /tmp/forge-install.sh"'
        )
        r = subprocess.run(
            ["docker", "exec", "-e", f"T={tok}", "-e", f"P={c['NODE_PASSWORD']}", n, "sh", "-c", script],
            capture_output=True, text=True,
        )
        step(f"{n} enrolled" if r.returncode == 0 else f"{n} enrollment failed: {r.stderr.strip()[-300:]}")

    # Wait for the agents.
    deadline = time.time() + 90
    while time.time() < deadline:
        servers = {s["hostname"]: s for s in api.get("/api/v1/servers")}
        if all(servers.get(n, {}).get("status") == "online" for n in ["node-1", "node-2", "node-3"]):
            break
        time.sleep(2)
    for n in ["node-1", "node-2", "node-3"]:
        step(f"{n}: {servers.get(n, {}).get('status', 'missing')}")

    # Cluster membership.
    placement = {"node-1": "lab", "node-2": "lab", "node-3": "uat"}
    for n, cl in placement.items():
        s = servers.get(n)
        if s and s.get("cluster_id") != clusters[cl]["id"]:
            api.patch(f"/api/v1/servers/{s['id']}", {"cluster_id": clusters[cl]["id"], "environment_id": None})
            step(f"{n} → cluster {cl}")

    # Network plans (subnet suggested by Forge).
    for cl in ["lab", "uat"]:
        cid = clusters[cl]["id"]
        if api.get(f"/api/v1/clusters/{cid}/fabric").get("spec"):
            step(f"network of {cl} exists")
            continue
        sug = api.post(f"/api/v1/clusters/{cid}/fabric/suggest", {"prefix": 16})
        api.put(f"/api/v1/clusters/{cid}/fabric", {"spec": {"subnet": sug["subnet"], "mode": "mesh", "domain": sug["domain"]}, "pins": []})
        step(f"network of {cl}: {sug['subnet']} ({sug['domain']})")

    # Environments (always inside a cluster) and services.
    environments = api.get("/api/v1/environments")

    def ns(name, cluster):
        cid = clusters[cluster]["id"]
        for x in environments:
            if x["name"] == name and x.get("cluster_id") == cid:
                return x
        x = api.post("/api/v1/environments", {"name": name, "protected": cluster == "lab" and name == "payments", "cluster_id": cid})
        environments.append(x)
        step(f"environment {name} in {cluster}")
        return x

    apps = api.get("/api/v1/services")

    def app(environment, name, runtime, **spec):
        if any(a["name"] == name and a["environment_id"] == environment["id"] for a in apps):
            return
        body = {
            "environment_id": environment["id"], "name": name, "runtime": runtime,
            "command": "", "working_dir": "", "image": "",
            "ports": [], "volumes": [], "env_files": [], "restart": True, "env": [],
        }
        body.update(spec)
        apps.append(api.post("/api/v1/services", body))
        step(f"service {name}.{environment['name']}")

    pay = ns("payments", "lab")
    shop = ns("shop", "lab")
    pay_uat = ns("payments-uat", "uat")
    app(pay, "api", "systemd", command="python3 -m http.server 8080", env=[{"key": "PORT", "value": "8080", "secret": False}])
    app(shop, "web", "container", image="nginx:alpine", ports=["8081:80"])
    app(pay_uat, "api", "systemd", command="python3 -m http.server 8080")

    # Re-plan the networks now that the services exist.
    for cl in ["lab", "uat"]:
        cid = clusters[cl]["id"]
        f = api.get(f"/api/v1/clusters/{cid}/fabric")
        api.put(f"/api/v1/clusters/{cid}/fabric", {"spec": f["spec"], "pins": f.get("pins") or []})

    # A vault credential linked to node-1 and the lab cluster.
    items = api.get("/api/v1/vault/items")
    if not any(i["name"] == "node-1 forgeops" for i in items) and "node-1" in servers:
        api.post("/api/v1/vault/items", {
            "name": "node-1 forgeops", "username": "forgeops", "password": c["NODE_PASSWORD"],
            "url": "ssh://node-1", "tags": ["lab", "ssh"],
            "links": [{"kind": "node", "id": servers["node-1"]["id"]}, {"kind": "cluster", "id": clusters["lab"]["id"]}],
            "notes": "Lab login created by scripts/lab/up.sh.",
        })
        step("vault credential for node-1")

    print("\nSeeded. Open http://localhost:8090 and pick a cluster in the rail.")


if __name__ == "__main__":
    sys.exit(main())
