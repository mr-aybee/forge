#!/usr/bin/env python3
"""Ingress test scenarios for the WSL lab, run end to end against the real
control plane and nodes. Run seed.py first.

    wsl -d Ubuntu-24.04 -- python3 scripts/lab/scenarios.py all
    wsl -d Ubuntu-24.04 -- python3 scripts/lab/scenarios.py portals|standalone|ha|failover|verify

Portals (Forge services in environment `portals` of cluster `lab`, placed
active on node-1 and node-2; each page says which node and IP served it):

    py-portal   Python 3     port 8101
    php-portal  PHP 8.3      port 8102
    net-portal  .NET 10 SDK  port 8103  (file-based app, first start builds)

Scenarios
    standalone  One Nginx edge (node-1). Endpoints py/php/net.lab.local map to
                the portals; requests through the edge must reach BOTH nodes
                (round-robin). No DNS.
    ha          Two Nginx edges (node-1, node-2), active-active, with PowerDNS
                on node-3 owning zone lab.local. Forge publishes A records
                listing both edges.
    failover    Pauses node-2 (link down): within ~60 s the records list only
                node-1; unpausing brings node-2 back.
    edge-standalone  A: Nginx standalone (node-1); portals active on node-1,
                standby on node-2; stop -> standby serves, start -> back.
    edge-aa     B: Nginx active-active (DNS both edges); portals active on both.
    edge-as     C: Nginx active-standby (DNS primary); portals active on both;
                py-portal twice on node-1 (8101 + 8111): balancing over three
                processes, restart/stop of one copy leaves the other alone.
    matrix      A, B and C in order.
    updates     Redis from apt on node-3, rolled back one security build:
                Forge must report the update and apply exactly that build.
    standby     Active-standby edges (node-1 primary, node-2 backup): DNS lists
                only the primary and moves to the backup when its link goes
                down. py-portal active on node-1, standby on node-2: all
                requests go to node-1 until its portal stops answering.

Lab-only shortcut: the portal sources are copied into the node containers
(/srv/forge-portals) with `docker cp`; on real nodes they would be deployed.
"""
import json
import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from seed import Api, creds, step  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
ZONE = "lab.local."
EDGE = "node-1"
EDGES = ["node-1", "node-2"]
APP_NODES = ["node-1", "node-2"]
DNS_NODE = "node-3"
PORTALS = [
    # name, port, domain, command, dependencies
    ("py-portal", 8101, "py.lab.local", "python3 /srv/forge-portals/portal.py",
     [{"package": "python", "version": "system"}]),
    ("php-portal", 8102, "php.lab.local", "php -S 0.0.0.0:8102 -t /srv/forge-portals/php",
     [{"package": "php", "version": "8.3"}]),
    ("net-portal", 8103, "net.lab.local", "dotnet run /srv/forge-portals/dotnet/portal.cs",
     [{"package": "dotnet", "version": "10.0", "params": {"flavor": "sdk"}}]),
]


def sh(*args, check=True):
    r = subprocess.run(list(args), capture_output=True, text=True)
    if check and r.returncode != 0:
        raise SystemExit(f"{' '.join(args)}: {r.stderr.strip() or r.stdout.strip()}")
    return r.stdout.strip()


def ip_of(node):
    return sh("docker", "inspect", "-f", "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}", node)


def ok(msg):
    print(f"  \033[32m✔\033[0m {msg}", flush=True)


def fail(msg):
    print(f"  \033[31m✘\033[0m {msg}", flush=True)
    return False


def wait(what, fn, timeout=600, every=5):
    deadline = time.time() + timeout
    while time.time() < deadline:
        v = fn()
        if v:
            return v
        time.sleep(every)
    raise SystemExit(f"timed out waiting for {what}")


class Lab:
    def __init__(self):
        c = creds()
        self.api = Api(c["ADMIN_EMAIL"], c["ADMIN_PASSWORD"])
        self.servers = {s["hostname"]: s for s in self.api.get("/api/v1/servers")}
        self.clusters = {c["name"]: c for c in self.api.get("/api/v1/clusters")}
        if "lab" not in self.clusters or any(n not in self.servers for n in APP_NODES + [DNS_NODE]):
            raise SystemExit("run scripts/lab/seed.py first")
        self.cid = self.clusters["lab"]["id"]

    def sid(self, node):
        return self.servers[node]["id"]

    # ------------------------------------------------------------ packages
    def installed(self, node, pkg):
        return any(
            i["server_id"] == self.sid(node) and i["package"] == pkg and i["status"] == "installed"
            for i in self.api.get("/api/v1/installations")
        )

    def ensure_package(self, pkg, version, nodes, params=None):
        todo = [n for n in nodes if not self.installed(n, pkg)]
        if not todo:
            step(f"{pkg} already on {', '.join(nodes)}")
            return
        self.api.post("/api/v1/packages/install", {"package": pkg, "version": version, "server_ids": [self.sid(n) for n in todo], "params": params or {}})
        step(f"installing {pkg} on {', '.join(todo)} …")
        for n in todo:
            def done(n=n):
                for i in self.api.get("/api/v1/installations"):
                    if i["server_id"] == self.sid(n) and i["package"] == pkg:
                        if i["status"] == "failed":
                            raise SystemExit(f"{pkg} failed on {n}: {i.get('last_error')}")
                        return i["status"] == "installed"
                return False
            wait(f"{pkg} on {n}", done, timeout=900)
        step(f"{pkg} installed")

    # ------------------------------------------------------------ portals
    def portals(self):
        print("\n== Portals")
        for n in APP_NODES:
            sh("docker", "exec", n, "mkdir", "-p", "/srv/forge-portals")
            sh("docker", "cp", os.path.join(HERE, "portals", "."), f"{n}:/srv/forge-portals/")
            sh("docker", "exec", n, "chmod", "-R", "a+rX", "/srv/forge-portals")
        step(f"portal sources copied to {', '.join(APP_NODES)}")

        ns = next((x for x in self.api.get(f"/api/v1/environments?cluster_id={self.cid}") if x["name"] == "portals"), None)
        if not ns:
            ns = self.api.post("/api/v1/environments", {"name": "portals", "protected": False, "cluster_id": self.cid})
            step("environment portals")
        apps = {a["name"]: a for a in self.api.get(f"/api/v1/services?environment_id={ns['id']}")}
        for name, port, _, command, deps in PORTALS:
            body = {
                "environment_id": ns["id"], "name": name, "runtime": "systemd", "command": command,
                "working_dir": "", "image": "", "ports": [], "volumes": [],
                "env_files": [], "restart": True, "dependencies": deps,
                "env": [{"key": "PORT", "value": str(port), "secret": False},
                        {"key": "DOTNET_CLI_TELEMETRY_OPTOUT", "value": "1", "secret": False},
                        {"key": "DOTNET_NOLOGO", "value": "1", "secret": False}],
            }
            if name in apps:
                app = self.api.put(f"/api/v1/services/{apps[name]['id']}", body)
            else:
                app = self.api.post("/api/v1/services", body)
                step(f"service {name}")
            self.api.put(f"/api/v1/services/{app['id']}/instances", [{"server_id": self.sid(n), "role": "active"} for n in APP_NODES])
            self.api.post(f"/api/v1/services/{app['id']}/deploy")
            step(f"{name}: deploying to {', '.join(APP_NODES)} (runtimes are installed first)")
            apps[name] = app

        def all_up():
            states = []
            for name, *_ in PORTALS:
                a = self.api.get(f"/api/v1/services/{apps[name]['id']}")
                for i in a["instances"]:
                    if i["status"] == "failed":
                        raise SystemExit(f"{name} failed on a node: {i.get('last_error')}")
                    states.append(i["status"] == "deployed")
            return all(states)
        wait("portal deploys", all_up, timeout=1500, every=10)
        # Each portal answers on each node directly.
        good = True
        for name, port, *_ in PORTALS:
            for n in APP_NODES:
                def answers(n=n, port=port):
                    out = subprocess.run(["docker", "exec", DNS_NODE, "curl", "-fsS", "-m", "3", f"http://{ip_of(n)}:{port}/?format=json"], capture_output=True, text=True)
                    return out.returncode == 0 and json.loads(out.stdout)
                info = wait(f"{name} on {n}", answers, timeout=300 if name == "net-portal" else 60, every=3)
                if info["node"] == n:
                    ok(f"{name} on {n}: {info['runtime']} at {', '.join(info['addresses'])}")
                else:
                    good = fail(f"{name} on {n} answered as {info['node']}")
        return good

    # ------------------------------------------------------------ ingress
    def apps_by_name(self):
        return {a["name"]: a for a in self.api.get(f"/api/v1/services?cluster_id={self.cid}")}

    def endpoints(self):
        apps = self.apps_by_name()
        sites = {s["name"]: s for s in self.api.get("/api/v1/sites")}
        for name, port, domain, *_ in PORTALS:
            body = {
                "name": name, "domains": [domain], "upstreams": [], "cluster_id": self.cid,
                "targets": [{"kind": "app", "service_id": apps[name]["id"], "port": port}],
                "balance": "round_robin", "tls": "off", "websockets": True, "max_body_mb": 25, "rate_limit": 0, "custom": "",
            }
            s = self.api.put(f"/api/v1/sites/{sites[name]['id']}", body) if name in sites else self.api.post("/api/v1/sites", body)
            r = self.api.post(f"/api/v1/sites/{s['id']}/apply")
            bad = [e for e in r["edges"] if e["result"]["status"] != "succeeded"]
            if bad:
                raise SystemExit(f"{domain}: apply failed: {bad[0]['result'].get('error')}")
            step(f"endpoint {domain} → {name}:{port}")

    def profile(self, mode, edges, dns=None):
        ingress = {
            "mode": mode,
            "edges": [{"server_id": self.sid(n), "role": "primary" if i == 0 else ("backup" if mode == "active_standby" else "primary"), "public_ip": ip_of(n)} for i, n in enumerate(edges)],
            "failover": "dns", "vrrp": None,
            "nginx": {"worker_connections": 1024, "keepalive_timeout_s": 65, "proxy_read_timeout_s": 60, "client_max_body_mb": 25,
                      "gzip": True, "server_tokens": False, "hsts": False, "real_ip_from": [], "rate_limit": 0, "access_log": "combined", "custom": ""},
        }
        if dns:
            ingress["dns"] = dns
        self.api.put(f"/api/v1/clusters/{self.cid}/profile", {"ingress": ingress})
        r = self.api.post(f"/api/v1/clusters/{self.cid}/profile/apply")
        bad = {k: v for k, v in r["edges"].items() if v["status"] != "applied"}
        if bad:
            raise SystemExit(f"profile apply failed: {bad}")
        step(f"profile {mode} on {', '.join(edges)} applied")
        return r

    def balance_check(self, edge):
        good = True
        for name, _, domain, *_ in PORTALS:
            seen = {}
            for _ in range(12):
                out = sh("docker", "exec", DNS_NODE, "curl", "-fsS", "-m", "5", "-H", f"Host: {domain}", f"http://{ip_of(edge)}/?format=json", check=False)
                try:
                    node = json.loads(out)["node"]
                except (ValueError, KeyError):
                    node = "error"
                seen[node] = seen.get(node, 0) + 1
            spread = ", ".join(f"{k}×{v}" for k, v in sorted(seen.items()))
            if set(seen) == set(APP_NODES):
                ok(f"{domain} via {edge}: {spread}")
            else:
                good = fail(f"{domain} via {edge}: {spread} (expected both {', '.join(APP_NODES)})")
        return good

    def standalone(self):
        print("\n== Scenario 1: one Nginx (standalone on node-1), round-robin to both nodes")
        self.ensure_package("nginx", "stable", [EDGE])
        self.profile("standalone", [EDGE])
        self.endpoints()
        return self.balance_check(EDGE)

    # ------------------------------------------------------------ DNS
    def dns_op(self, op):
        r = self.api.post(f"/api/v1/servers/{self.sid(DNS_NODE)}/dns", op)
        if r["status"] != "succeeded":
            raise SystemExit(f"DNS {op['op']}: {r.get('error')}")
        return r.get("result")

    def a_records(self, domain):
        recs = (self.dns_op({"op": "records", "zone": ZONE}) or {}).get("records", [])
        for r in recs:
            if r["name"] == f"{domain}." and r["type"] == "A":
                return sorted(r["contents"])
        return []

    def ha(self):
        print("\n== Scenario 2: two Nginx (active-active) + PowerDNS zone lab.local on node-3")
        self.ensure_package("nginx", "stable", EDGES)
        self.ensure_package("powerdns", "distro", [DNS_NODE])
        zones = [z["name"] for z in (self.dns_op({"op": "zones"}) or [])]
        if ZONE not in zones:
            self.dns_op({"op": "create_zone", "zone": ZONE, "nameservers": ["ns1.lab.local."]})
            self.dns_op({"op": "set_records", "zone": ZONE, "name": "ns1.lab.local.", "rtype": "A", "ttl": 300, "contents": [ip_of(DNS_NODE)]})
            step("zone lab.local created with ns1")
        r = self.profile("active_active", EDGES, {"server_id": self.sid(DNS_NODE), "ttl": 30, "auto_failover": True})
        if r.get("dns", {}).get("error"):
            raise SystemExit(f"DNS publish failed: {r['dns']['error']}")
        self.endpoints()
        want = sorted(ip_of(n) for n in EDGES)
        good = True
        for _, _, domain, *_ in PORTALS:
            got = self.a_records(domain)
            if got == want:
                ok(f"{domain} A {', '.join(got)}")
            else:
                good = fail(f"{domain} A {got} (expected {want})")
        for n in EDGES:
            good &= self.balance_check(n)
        return good

    def failover(self):
        print("\n== Scenario 3: link to node-2 goes down, DNS moves, then comes back")
        domain = PORTALS[0][2]
        both = sorted(ip_of(n) for n in EDGES)
        only1 = [ip_of("node-1")]
        if self.a_records(domain) != both:
            return fail(f"{domain} should list both edges before the test (run `ha` first)")
        sh("docker", "pause", "node-2")
        step("node-2 paused (link down); waiting for 3 failed checks …")
        try:
            t0 = time.time()
            wait("records without node-2", lambda: self.a_records(domain) == only1, timeout=180, every=5)
            ok(f"{domain} A {only1[0]} after {time.time() - t0:.0f}s")
        finally:
            sh("docker", "unpause", "node-2")
        step("node-2 unpaused; waiting for 3 good checks …")
        t0 = time.time()
        wait("records with node-2 again", lambda: self.a_records(domain) == both, timeout=240, every=5)
        ok(f"{domain} A {', '.join(both)} after {time.time() - t0:.0f}s")
        audit = self.api.get("/api/v1/audit?limit=100")
        moves = [e for e in (audit if isinstance(audit, list) else audit.get("entries", [])) if e.get("action") == "ingress.dns.failover"]
        (ok if len(moves) >= 2 else fail)(f"{len(moves)} failover events in the audit log")
        return len(moves) >= 2


def standby(self):
    """Scenario 4: active-standby at both levels.

    Edges: node-1 primary, node-2 backup; DNS lists only the primary and
    moves to the backup when the primary's link goes down.
    Service: py-portal active on node-1, standby on node-2; Nginx sends
    everything to node-1 and only uses node-2 when node-1's portal is down.
    """
    print("\n== Scenario 4: active-standby edges (DNS) and an active-standby service")
    good = True
    domain = PORTALS[0][2]
    p1, p2 = ip_of("node-1"), ip_of("node-2")
    r = self.profile("active_standby", EDGES, {"server_id": self.sid(DNS_NODE), "ttl": 30, "auto_failover": True})
    if r.get("dns", {}).get("error"):
        raise SystemExit(f"DNS publish failed: {r['dns']['error']}")
    got = wait("records on the primary only", lambda: self.a_records(domain) == [p1] and self.a_records(domain), timeout=60)
    ok(f"{domain} A {got[0]} (primary node-1 only)")

    # Service placement: active on node-1, standby on node-2.
    app = self.apps_by_name()["py-portal"]
    self.api.put(f"/api/v1/services/{app['id']}/instances", [
        {"server_id": self.sid("node-1"), "role": "active"}, {"server_id": self.sid("node-2"), "role": "standby"}])
    self.endpoints()  # re-resolve: node-2 becomes a backup upstream
    seen = {}
    for _ in range(10):
        out = sh("docker", "exec", DNS_NODE, "curl", "-fsS", "-m", "5", "-H", f"Host: {domain}", f"http://{p1}/?format=json", check=False)
        node = json.loads(out)["node"] if out.startswith("{") else "error"
        seen[node] = seen.get(node, 0) + 1
    if set(seen) == {"node-1"}:
        ok(f"{domain}: all 10 requests to the active node-1")
    else:
        good = fail(f"{domain}: {seen} (expected only node-1)")
    # Stop the active instance through Forge, as an operator or a crash would.
    act = f"/api/v1/services/{app['id']}/instances/{self.sid('node-1')}/actions"
    self.api.post(act, {"action": "stop"})
    try:
        time.sleep(2)
        out = sh("docker", "exec", DNS_NODE, "curl", "-fsS", "-m", "15", "-H", f"Host: {domain}", f"http://{p1}/?format=json", check=False)
        node = json.loads(out)["node"] if out.startswith("{") else "error"
        (ok if node == "node-2" else fail)(f"{domain} with node-1's portal stopped: served by {node} (standby)")
        good &= node == "node-2"
    finally:
        self.api.post(act, {"action": "start"})

    # Edge failover: primary link down -> DNS moves to the backup.
    sh("docker", "pause", "node-1")
    step("node-1 (primary edge) paused; waiting for DNS to move to node-2 …")
    try:
        t0 = time.time()
        wait("records on the backup", lambda: self.a_records(domain) == [p2], timeout=180, every=5)
        ok(f"{domain} A {p2} (backup) after {time.time() - t0:.0f}s")
    finally:
        sh("docker", "unpause", "node-1")
    t0 = time.time()
    wait("records back on the primary", lambda: self.a_records(domain) == [p1], timeout=240, every=5)
    ok(f"{domain} A {p1} (primary again) after {time.time() - t0:.0f}s")

    # Restore: both nodes active again for the other scenarios.
    self.api.put(f"/api/v1/services/{app['id']}/instances", [{"server_id": self.sid(n), "role": "active"} for n in APP_NODES])
    self.endpoints()
    return good


Lab.standby = standby


UPD_NODE = "node-3"
REDIS_OLD = "5:7.0.15-1~deb12u7"  # bookworm release; bookworm-security is newer


def updates(self):
    """Install Redis from apt, roll it back one security build on the node,
    then let Forge find and apply the update (exact build, service restarted,
    running version verified)."""
    print("\n== Package updates (Redis on node-3)")
    self.ensure_package("redis", "distro", [UPD_NODE])
    inst = next(i for i in self.api.get("/api/v1/installations")
                if i["server_id"] == self.sid(UPD_NODE) and i["package"] == "redis")
    sh("docker", "exec", UPD_NODE, "sh", "-c",
       f"apt-get install -y -qq --allow-downgrades redis-server={REDIS_OLD} redis-tools={REDIS_OLD} >/dev/null 2>&1"
       " && service redis-server restart >/dev/null 2>&1")
    step(f"rolled redis-server back to {REDIS_OLD} on the node")

    def row():
        return next((r for r in self.api.get("/api/v1/package-updates") if r["installation_id"] == inst["id"]), None)

    since = time.time() * 1000
    self.api.post("/api/v1/package-updates/check", {})
    step("asked the nodes for their builds ...")
    wait("a fresh version report", lambda: ((row() or {}).get("checked_at") or 0) >= since, timeout=240)
    r = row()
    good = r["installed"] == REDIS_OLD and bool(r["kind"]) and r["candidate"] != REDIS_OLD
    (ok if good else fail)(f"installed {r['installed']} -> available {r['candidate']} ({r['kind']}) via {r['manager']}")
    up = r.get("upstream")
    step(f"vendor line {up['line']}: latest {up['latest']}, lts={up['lts']}, eol={up['eol']}" if up else "vendor line: none")

    # Only the build the node offers is accepted (the update is pinned).
    try:
        self.api.post(f"/api/v1/installations/{inst['id']}/update", {"build": "5:7.0.15-1~deb12u1"})
        fail("an update to a build the node does not offer was accepted")
        good = False
    except SystemExit as e:
        ok(f"a build the node does not offer is refused ({str(e).split(': ', 1)[-1][:70]})")

    target = r["candidate"]
    self.api.post(f"/api/v1/installations/{inst['id']}/update", {"build": target})
    step(f"updating to {target} ...")
    wait("the update to finish", lambda: not row()["status"].endswith("ing"), timeout=600)
    r = row()
    after = sh("docker", "exec", UPD_NODE, "dpkg-query", "-W", "-f=${Version}", "redis-server")
    good &= after == target and r["installed"] == target and not r["kind"]
    (ok if after == target else fail)(f"node has redis-server {after}")
    (ok if r["running"] else fail)(f"service reports {r['running'] or 'nothing'}; Forge shows {r['installed']}, status {r['status']}")
    audit = self.api.get("/api/v1/audit?limit=50")
    ev = [e for e in (audit if isinstance(audit, list) else audit.get("entries", [])) if e.get("action") == "package.update"]
    (ok if ev else fail)(f"{len(ev)} package.update event(s) in the audit log")
    return good and bool(ev)


Lab.updates = updates


# ---------------------------------------------------------------- edge × service matrix
TWIN = ("py-portal-2", 8111)  # a second copy of py-portal on node-1 only (scenario C)


def hits(self, edge, domain, n=12):
    """Sends n requests through an edge; counts (node, port) that answered."""
    seen = {}
    for _ in range(n):
        out = sh("docker", "exec", DNS_NODE, "curl", "-fsS", "-m", "15", "-H", f"Host: {domain}",
                 f"http://{ip_of(edge)}/?format=json", check=False)
        try:
            j = json.loads(out)
            key = f"{j['node']}:{j['port']}"
        except (ValueError, KeyError):
            key = "error"
        seen[key] = seen.get(key, 0) + 1
    return seen


def spread(seen):
    return ", ".join(f"{k}×{v}" for k, v in sorted(seen.items()))


def place(self, roles):
    """roles: {node: "active"|"standby"} for every portal; re-resolves endpoints."""
    for name, *_ in PORTALS:
        app = self.apps_by_name()[name]
        self.api.put(f"/api/v1/services/{app['id']}/instances",
                     [{"server_id": self.sid(n), "role": r} for n, r in roles.items()])
    step("portals placed: " + ", ".join(f"{n} {r}" for n, r in roles.items()))


def instance_action(self, app_name, node, action):
    app = self.apps_by_name()[app_name]
    self.api.post(f"/api/v1/services/{app['id']}/instances/{self.sid(node)}/actions", {"action": action})


def ensure_dns_zone(self):
    self.ensure_package("powerdns", "distro", [DNS_NODE])
    zones = [z["name"] for z in (self.dns_op({"op": "zones"}) or [])]
    if ZONE not in zones:
        self.dns_op({"op": "create_zone", "zone": ZONE, "nameservers": ["ns1.lab.local."]})
        self.dns_op({"op": "set_records", "zone": ZONE, "name": "ns1.lab.local.", "rtype": "A", "ttl": 300,
                     "contents": [ip_of(DNS_NODE)]})
    return {"server_id": self.sid(DNS_NODE), "ttl": 30, "auto_failover": True}


def scenario_a(self):
    """A: one Nginx (standalone, node-1); portals active on node-1, standby
    on node-2. Everything goes to node-1 until its portal stops, then node-2
    takes over, and node-1 takes it back after a start."""
    print("\n== Scenario A: Nginx standalone · portals active on node-1, standby on node-2")
    good = True
    self.ensure_package("nginx", "stable", [EDGE])
    self.profile("standalone", [EDGE])
    self.place({"node-1": "active", "node-2": "standby"})
    self.endpoints()
    for name, port, domain, *_ in PORTALS:
        seen = self.hits(EDGE, domain, 10)
        want = {f"node-1:{port}"}
        (ok if set(seen) == want else fail)(f"{domain}: {spread(seen)} (active node-1 only)")
        good &= set(seen) == want
    name, port, domain, *_ = PORTALS[0]
    self.instance_action(name, "node-1", "stop")
    try:
        time.sleep(2)
        seen = self.hits(EDGE, domain, 6)
        want = {f"node-2:{port}"}
        (ok if set(seen) == want else fail)(f"{domain} with node-1's {name} stopped: {spread(seen)} (standby node-2)")
        good &= set(seen) == want
    finally:
        self.instance_action(name, "node-1", "start")
    back = wait(f"{name} on node-1 again", lambda: set(self.hits(EDGE, domain, 4)) == {f"node-1:{port}"} and True,
                timeout=90, every=3)
    ok(f"{domain}: back on node-1 after the start") if back else None
    return good


def scenario_b(self):
    """B: two Nginx active-active (DNS lists both); portals active on both
    nodes. Each edge spreads requests over both nodes."""
    print("\n== Scenario B: Nginx active-active · portals active on node-1 and node-2")
    good = True
    self.ensure_package("nginx", "stable", EDGES)
    dns = self.ensure_dns_zone()
    r = self.profile("active_active", EDGES, dns)
    if r.get("dns", {}).get("error"):
        raise SystemExit(f"DNS publish failed: {r['dns']['error']}")
    self.place({n: "active" for n in APP_NODES})
    self.endpoints()
    both = sorted(ip_of(n) for n in EDGES)
    for name, port, domain, *_ in PORTALS:
        got = wait(f"{domain} A records", lambda d=domain: self.a_records(d) == both and both, timeout=90)
        ok(f"{domain} A {', '.join(got)} (both edges)")
        for edge in EDGES:
            seen = self.hits(edge, domain, 12)
            want = {f"{n}:{port}" for n in APP_NODES}
            (ok if set(seen) == want else fail)(f"{domain} via {edge}: {spread(seen)}")
            good &= set(seen) == want
    return good


def ensure_twin(self):
    """A second py-portal process on node-1 (own service, port 8111)."""
    apps = self.apps_by_name()
    base = apps["py-portal"]
    name, port = TWIN
    body = {
        "environment_id": base["environment_id"], "name": name, "runtime": "systemd",
        "command": "python3 /srv/forge-portals/portal.py", "working_dir": "", "image": "",
        "ports": [], "volumes": [], "env_files": [], "restart": True,
        "dependencies": [{"package": "python", "version": "system"}],
        "env": [{"key": "PORT", "value": str(port), "secret": False}],
    }
    app = self.api.put(f"/api/v1/services/{apps[name]['id']}", body) if name in apps \
        else self.api.post("/api/v1/services", body)
    self.api.put(f"/api/v1/services/{app['id']}/instances", [{"server_id": self.sid("node-1"), "role": "active"}])
    self.api.post(f"/api/v1/services/{app['id']}/deploy")
    step(f"{name}: second copy of py-portal on node-1, port {port}")

    def up():
        a = self.api.get(f"/api/v1/services/{app['id']}")
        if any(i["status"] == "failed" for i in a["instances"]):
            raise SystemExit(f"{name} failed: {a['instances'][0].get('last_error')}")
        return all(i["status"] == "deployed" for i in a["instances"])
    wait(f"{name} deploy", up, timeout=300, every=5)
    return app


def direct(node, port):
    out = sh("docker", "exec", DNS_NODE, "curl", "-fsS", "-m", "5", f"http://{ip_of(node)}:{port}/?format=json", check=False)
    try:
        return json.loads(out)
    except ValueError:
        return None


def scenario_c(self):
    """C: two Nginx active-standby (DNS lists the primary); portals active on
    both nodes; py-portal runs twice on node-1 (ports 8101 and 8111). The
    primary edge spreads over all three processes; stopping, restarting and
    starting one copy must not touch the other."""
    print("\n== Scenario C: Nginx active-standby · portals active on both nodes · py-portal twice on node-1")
    good = True
    self.ensure_package("nginx", "stable", EDGES)
    dns = self.ensure_dns_zone()
    r = self.profile("active_standby", EDGES, dns)
    if r.get("dns", {}).get("error"):
        raise SystemExit(f"DNS publish failed: {r['dns']['error']}")
    self.place({n: "active" for n in APP_NODES})
    twin = self.ensure_twin()
    self.endpoints()
    # py.lab.local balances over both py-portal services.
    name, port, domain, *_ = PORTALS[0]
    apps = self.apps_by_name()
    site = next(s for s in self.api.get("/api/v1/sites") if s["name"] == name)
    body = {
        "name": name, "domains": [domain], "upstreams": [], "cluster_id": self.cid,
        "targets": [{"kind": "app", "service_id": apps[name]["id"], "port": port},
                    {"kind": "app", "service_id": twin["id"], "port": TWIN[1]}],
        "balance": "round_robin", "tls": "off", "websockets": True, "max_body_mb": 25, "rate_limit": 0, "custom": "",
    }
    self.api.put(f"/api/v1/sites/{site['id']}", body)
    self.api.post(f"/api/v1/sites/{site['id']}/apply")
    step(f"endpoint {domain} → {name}:{port} + {TWIN[0]}:{TWIN[1]}")

    p1 = ip_of("node-1")
    got = wait(f"{domain} A on the primary only", lambda: self.a_records(domain) == [p1] and [p1], timeout=90)
    ok(f"{domain} A {got[0]} (primary node-1 only)")

    three = {f"node-1:{port}", f"node-1:{TWIN[1]}", f"node-2:{port}"}
    seen = self.hits("node-1", domain, 18)
    (ok if set(seen) == three else fail)(f"{domain} via primary: {spread(seen)}")
    good &= set(seen) == three
    for other, oport, odomain, *_ in PORTALS[1:]:
        seen = self.hits("node-1", odomain, 10)
        want = {f"{n}:{oport}" for n in APP_NODES}
        (ok if set(seen) == want else fail)(f"{odomain} via primary: {spread(seen)}")
        good &= set(seen) == want

    # Process management: the two copies on node-1 are separate processes.
    a, b = direct("node-1", port), direct("node-1", TWIN[1])
    if not (a and b) or a["pid"] == b["pid"]:
        return fail(f"two copies on node-1 should be two processes: {a and a['pid']} / {b and b['pid']}")
    ok(f"node-1 runs py-portal (pid {a['pid']}) and {TWIN[0]} (pid {b['pid']})")

    self.instance_action(TWIN[0], "node-1", "restart")
    nb = wait(f"{TWIN[0]} restarted", lambda: (x := direct("node-1", TWIN[1])) and x["pid"] != b["pid"] and x, timeout=60, every=2)
    na = direct("node-1", port)
    same = na and na["pid"] == a["pid"]
    (ok if same else fail)(f"restart {TWIN[0]}: new pid {nb['pid']}; py-portal untouched (pid {na and na['pid']})")
    good &= bool(same)

    self.instance_action(TWIN[0], "node-1", "stop")
    try:
        wait(f"{TWIN[0]} stopped", lambda: direct("node-1", TWIN[1]) is None, timeout=30, every=1)
        na = direct("node-1", port)
        (ok if na and na["pid"] == a["pid"] else fail)(f"stop {TWIN[0]}: py-portal on node-1 still serving (pid {na and na['pid']})")
        good &= bool(na and na["pid"] == a["pid"])
        seen = self.hits("node-1", domain, 12)
        want = {f"node-1:{port}", f"node-2:{port}"}
        (ok if set(seen) == want else fail)(f"{domain} with {TWIN[0]} stopped: {spread(seen)} (no errors)")
        good &= set(seen) == want
    finally:
        self.instance_action(TWIN[0], "node-1", "start")
    wait(f"{TWIN[0]} back", lambda: direct("node-1", TWIN[1]) is not None, timeout=60, every=2)
    seen = self.hits("node-1", domain, 18)
    (ok if set(seen) == three else fail)(f"{domain} after start: {spread(seen)}")
    good &= set(seen) == three

    # The other way round: stopping py-portal on node-1 leaves the twin running.
    self.instance_action(name, "node-1", "stop")
    try:
        wait(f"{name} on node-1 stopped", lambda: direct("node-1", port) is None, timeout=30, every=1)
        nb2 = direct("node-1", TWIN[1])
        (ok if nb2 else fail)(f"stop py-portal on node-1: {TWIN[0]} still serving (pid {nb2 and nb2['pid']})")
        good &= bool(nb2)
    finally:
        self.instance_action(name, "node-1", "start")
    wait(f"{name} on node-1 back", lambda: direct("node-1", port) is not None, timeout=60, every=2)
    return good


for _f in (hits, place, instance_action, ensure_dns_zone, scenario_a, scenario_b, ensure_twin, scenario_c):
    setattr(Lab, _f.__name__, _f)


def names(self):
    """Cluster names: every node has the Forge hosts block; node and service
    names resolve to mesh addresses and answer across the mesh; the
    container logs `since` cursor returns only newer lines."""
    print("\n== Cluster names (*.forge.internal) and live container logs")
    good = True
    domain = self.api.get(f"/api/v1/clusters/{self.cid}/fabric")["spec"]["domain"]
    want = f"py-portal.portals.{domain}"

    def block(node):
        return sh("docker", "exec", node, "sh", "-c", "sed -n '/# BEGIN orbyn-forge/,/# END orbyn-forge/p' /etc/hosts", check=False)

    for n in APP_NODES + [DNS_NODE]:
        b = wait(f"hosts block on {n}", lambda n=n: want in block(n) and block(n), timeout=120, every=5)
        lines = [l for l in b.splitlines() if l and not l.startswith("#")]
        ok(f"{n}: {len(lines)} names in the Forge hosts block")
    ips = sorted(set(sh("docker", "exec", DNS_NODE, "getent", "ahostsv4", want).split()[::3]))
    ok(f"{want} -> {', '.join(ips)}")
    out = sh("docker", "exec", DNS_NODE, "curl", "-fsS", "-m", "5", f"http://{want}:8101/?format=json", check=False)
    try:
        j = json.loads(out)
        ok(f"curl {want}:8101 answered from {j['node']}")
    except ValueError:
        good = fail(f"curl {want}:8101 failed: {out[:120]}")
    host = f"node-1.{domain}"
    out = sh("docker", "exec", DNS_NODE, "getent", "hosts", host, check=False)
    (ok if out else fail)(f"{host} -> {out.split()[0] if out else 'unresolved'}")
    good &= bool(out)

    # Live container logs: a container that prints a line per second.
    sid = self.sid("node-1")
    sh("docker", "exec", "node-1", "sh", "-c",
       "command -v docker >/dev/null || exit 0; docker rm -f forge-logtest >/dev/null 2>&1; "
       "docker run -d --name forge-logtest busybox sh -c 'i=0; while true; do i=$((i+1)); echo tick $i; sleep 1; done' >/dev/null")
    if "forge-logtest" not in sh("docker", "exec", "node-1", "sh", "-c", "docker ps --format '{{.Names}}' 2>/dev/null", check=False):
        step("no Docker on node-1; skipping the container logs check")
        return good
    try:
        time.sleep(4)
        r = self.api.post(f"/api/v1/servers/{sid}/containers", {"op": "logs", "id": "forge-logtest", "tail": 100})
        first = r["result"]["logs"].strip().splitlines()
        last_ts = first[-1].split()[0]
        secs = time.mktime(time.strptime(last_ts[:19], "%Y-%m-%dT%H:%M:%S")) - time.timezone
        frac = (last_ts[20:-1] if "." in last_ts else "").ljust(9, "0")
        cursor = f"{int(secs)}.{frac}"
        sh("docker", "exec", "node-1", "docker", "restart", "-t", "1", "forge-logtest")
        time.sleep(3)
        r = self.api.post(f"/api/v1/servers/{sid}/containers", {"op": "logs", "id": "forge-logtest", "tail": 5000, "since": cursor})
        newer = [l for l in r["result"]["logs"].strip().splitlines() if l.split()[0] > last_ts]
        restarted = any(l.endswith("tick 1") for l in newer)
        (ok if newer and restarted else fail)(f"since cursor: {len(newer)} new lines after the restart, counter restarted: {restarted}")
        good &= bool(newer) and restarted
        try:
            self.api.post(f"/api/v1/servers/{sid}/containers", {"op": "logs", "id": "forge-logtest", "tail": 10, "since": "1&until=2"})
            good = fail("a malformed since was accepted")
        except SystemExit:
            ok("a malformed since is refused")
    finally:
        sh("docker", "exec", "node-1", "docker", "rm", "-f", "forge-logtest", check=False)
    return good


Lab.names = names


def main():
    what = sys.argv[1] if len(sys.argv) > 1 else "all"
    lab = Lab()
    plan = {
        "portals": [lab.portals],
        "standalone": [lab.standalone],
        "ha": [lab.ha],
        "failover": [lab.failover],
        "standby": [lab.standby],
        "updates": [lab.updates],
        "names": [lab.names],
        "edge-standalone": [lab.scenario_a],
        "edge-aa": [lab.scenario_b],
        "edge-as": [lab.scenario_c],
        "matrix": [lab.scenario_a, lab.scenario_b, lab.scenario_c],
        "verify": [lambda: lab.balance_check(EDGE)],
        "all": [lab.portals, lab.standalone, lab.ha, lab.failover, lab.standby],
    }.get(what)
    if not plan:
        raise SystemExit(__doc__)
    results = [f() for f in plan]
    print("\nPASS" if all(results) else "\nFAIL")
    return 0 if all(results) else 1


if __name__ == "__main__":
    sys.exit(main())
