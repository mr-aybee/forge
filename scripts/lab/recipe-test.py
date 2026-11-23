#!/usr/bin/env python3
"""Runs App Store recipes for real on systemd containers of each supported
distribution family, the way the node helper runs them (prelude + recipe,
`sh -s -- <action>`, a clean environment with the validated variables).

    wsl -d Ubuntu-24.04 -- python3 scripts/lab/recipe-test.py                 # every package, every distro
    wsl -d Ubuntu-24.04 -- python3 scripts/lab/recipe-test.py rocky9 php      # one distro, some packages
    wsl -d Ubuntu-24.04 -- python3 scripts/lab/recipe-test.py --keep debian12 redis
    wsl -d Ubuntu-24.04 -- python3 scripts/lab/recipe-test.py --heavy rocky9 gitlab-ce   # 4 GB RAM, slow

For each package: install -> status -> versions (installed, candidate,
running) -> upgrade (to the current candidate: a no-op that must succeed)
-> restart -> status -> uninstall. A fresh container per distro; packages run
in order (certbot needs nginx). Results go to .lab/recipe-test.json.
"""
import json
import os
import subprocess
import sys
import time

CERTBOT_NEXT = False
HERE = os.path.dirname(os.path.abspath(__file__))
RECIPES = os.path.join(HERE, "..", "..", "src", "protocol", "forge-catalog", "recipes")

DISTROS = {
    "debian12": ("debian:bookworm", "apt",
                 "RUN apt-get update && apt-get install -y --no-install-recommends systemd systemd-sysv dbus ca-certificates curl procps iproute2 && rm -rf /var/lib/apt/lists/*\n"
                 "CMD [\"/lib/systemd/systemd\"]\n"),
    "ubuntu24": ("ubuntu:24.04", "apt",
                 "RUN apt-get update && apt-get install -y --no-install-recommends systemd systemd-sysv dbus ca-certificates curl procps iproute2 && rm -rf /var/lib/apt/lists/*\n"
                 "CMD [\"/lib/systemd/systemd\"]\n"),
    "rocky9": ("rockylinux/rockylinux:9-ubi-init", "dnf",
               "RUN dnf install -y --allowerasing procps-ng iproute curl && dnf clean all\n"),
}

# package, version, params, has a service that reports a running version
CASES = [
    ("python", "system", {}, True),
    ("nginx", "stable", {}, True),
    ("certbot", "distro", {}, True),
    ("postgresql", "17", {"port": "5432", "bind": "localhost"}, True),
    ("redis", "distro", {"port": "6379", "bind": "localhost"}, True),
    ("mariadb", "distro", {"port": "3306", "bind": "localhost"}, True),
    ("mongodb", "8.0", {"port": "27017", "bind": "localhost"}, True),
    ("nodejs", "22", {}, True),
    ("php", "8.3", {"extensions": "mbstring,xml,curl,zip,intl,gd,mysql,pgsql,redis,opcache"}, True),
    ("java", "21", {}, True),
    ("dotnet", "8.0", {"flavor": "aspnetcore"}, True),
    ("powerdns", "distro", {"api_port": "8081"}, True),
    ("keepalived", "distro", {"vip": "10.99.0.50", "state": "MASTER", "interface": "eth0"}, True),
    ("docker", "stable", {}, True),
    ("mysql", "8.4", {"port": "3307", "bind": "localhost"}, True),
    ("valkey", "distro", {"port": "6380", "bind": "localhost"}, True),
    ("memcached", "distro", {"port": "11211", "memory_mb": "64", "bind": "localhost"}, True),
    ("clickhouse", "lts", {"port": "9000", "http_port": "8123", "bind": "localhost"}, True),
    ("influxdb", "2", {"port": "8086", "bind": "localhost"}, True),
    ("elasticsearch", "8", {"port": "9200", "heap": "512m", "bind": "localhost"}, True),
    ("opensearch", "2", {"port": "9201", "heap": "512m", "bind": "localhost"}, True),
    ("rabbitmq", "stable", {"port": "5672", "ui_port": "15672", "bind": "localhost"}, True),
    ("mosquitto", "distro", {"port": "1883", "bind": "localhost"}, True),
    ("apache", "distro", {"port": "8081", "bind": "localhost"}, True),
    ("haproxy", "distro", {}, True),
    ("caddy", "stable", {"port": "8082", "bind": "localhost"}, True),
    ("tomcat", "distro", {"port": "8080", "bind": "localhost"}, True),
    ("grafana", "stable", {"port": "3000", "bind": "localhost"}, True),
    ("loki", "stable", {"port": "3100", "bind": "localhost"}, True),
    ("prometheus", "distro", {"port": "9090", "bind": "localhost"}, True),
    ("node-exporter", "distro", {"port": "9100", "bind": "localhost"}, True),
    ("zabbix-agent", "7.0", {"server": "127.0.0.1"}, True),
    ("jenkins", "lts", {"port": "8085", "bind": "localhost"}, True),
]
HEAVY = [("gitlab-ce", "stable", {"port": "8929", "host": "localhost"}, True)]


def sh(*a, check=True, input=None, timeout=None):
    r = subprocess.run(list(a), capture_output=True, text=True, input=input, timeout=timeout)
    if check and r.returncode:
        raise SystemExit(f"{' '.join(a[:4])}: {r.stderr.strip() or r.stdout.strip()}")
    return r


def script(pkg):
    prelude = open(os.path.join(RECIPES, "_prelude.sh"), encoding="utf-8").read()
    recipe = open(os.path.join(RECIPES, f"{pkg}.sh"), encoding="utf-8").read()
    return f"{prelude}\n# ---- recipe: {pkg} ----\n{recipe}\nforge_dispatch \"$@\"\n"


def image(distro):
    base, _, extra = DISTROS[distro]
    tag = f"forge-recipe-{distro}"
    sh("docker", "build", "-q", "-t", tag, "-", input=f"FROM {base}\nENV container=docker\n{extra}", timeout=1800)
    return tag


def start(distro):
    name = f"recipe-{distro}"
    sh("docker", "rm", "-f", name, check=False)
    sh("docker", "run", "-d", "--name", name, "--privileged", "--cgroupns=host",
       "-v", "/sys/fs/cgroup:/sys/fs/cgroup:rw", "--tmpfs", "/run", "--tmpfs", "/run/lock", image(distro))
    for _ in range(60):
        st = sh("docker", "exec", name, "systemctl", "is-system-running", check=False).stdout.strip()
        if st in ("running", "degraded"):
            return name
        time.sleep(1)
    raise SystemExit(f"{distro}: systemd did not come up")


def run(name, pkg, version, params, action, build=None):
    env = ["FORGE_PKG_ID=" + pkg, "FORGE_PKG_VERSION=" + version]
    env += [f"FORGE_P_{k.upper()}={v}" for k, v in params.items()]
    if build:
        env.append(f"FORGE_PKG_BUILD={build}")
    cmd = ["docker", "exec", "-i", name, "env", "-i", "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
           "HOME=/root", "LANG=C.UTF-8", *env, "/bin/sh", "-s", "--", action]
    r = subprocess.run(cmd, capture_output=True, text=True, input=script(pkg), timeout=2700)
    last = next((l.strip() for l in reversed(r.stdout.splitlines()) if l.strip()), "")
    return r.returncode == 0, last, (r.stdout + r.stderr)[-1500:]


def check_pkg(name, distro, pkg, version, params, service):
    res = {"package": pkg, "distro": distro, "steps": {}}
    t0 = time.time()

    def step(action, build=None):
        ok, last, out = run(name, pkg, version, params, action, build)
        res["steps"][action] = {"ok": ok, "last": last}
        if not ok:
            res["error"] = f"{action}: {out.strip().splitlines()[-1] if out.strip() else 'failed'}"
            res["output"] = out
        return ok, last

    ok, last = step("install")
    if ok:
        ok, st = step("status")
        ok = ok and st == "running"
        if not ok and "error" not in res:
            res["error"] = f"status after install: {st}"
    if ok:
        ok, last = step("versions")
        try:
            v = json.loads(last)
        except ValueError:
            v, ok = {}, False
            res["error"] = f"versions printed: {last[:120]}"
        res["versions"] = v
        if ok and v.get("manager") not in ("apt", "dnf", "yum", "manual"):
            ok, res["error"] = False, f"manager {v.get('manager')}"
        if ok and v.get("manager") != "manual" and not v.get("installed"):
            ok, res["error"] = False, "no installed build reported"
        if ok and service and v.get("manager") != "manual" and not v.get("running"):
            ok, res["error"] = False, "the service reported no running version"
    if ok and res["versions"].get("manager") != "manual":
        cand = res["versions"].get("candidate") or res["versions"].get("installed")
        ok, last = step("upgrade", cand)
    if ok:
        ok, last = step("restart")
        if ok:
            ok, st = step("status")
            ok = st == "running"
            if not ok:
                res["error"] = f"status after restart: {st}"
    # certbot needs nginx: keep nginx until certbot has run.
    keep = "--keep" in sys.argv or (pkg == "nginx" and CERTBOT_NEXT)
    if not keep:
        u_ok, last = step("uninstall")
        ok = ok and u_ok
    res["ok"] = ok
    res["seconds"] = round(time.time() - t0)
    return res


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    distros = [a for a in args if a in DISTROS] or list(DISTROS)
    pkgs = [a for a in args if a not in DISTROS]
    all_cases = CASES + (HEAVY if "--heavy" in sys.argv or any(p in pkgs for p, *_ in HEAVY) else [])
    cases = [c for c in all_cases if not pkgs or c[0] in pkgs]
    # certbot needs nginx on the node.
    if any(c[0] == "certbot" for c in cases) and not any(c[0] == "nginx" for c in cases):
        cases.insert(0, next(c for c in CASES if c[0] == "nginx"))
    results = []
    for d in distros:
        print(f"\n== {d}", flush=True)
        name = start(d)
        try:
            global CERTBOT_NEXT
            CERTBOT_NEXT = any(c[0] == "certbot" for c in cases)
            for pkg, version, params, service in cases:
                r = check_pkg(name, d, pkg, version, params, service)
                if pkg == "certbot" and CERTBOT_NEXT:
                    run(name, "nginx", "stable", {}, "uninstall")
                results.append(r)
                v = r.get("versions") or {}
                mark = "\033[32m✔\033[0m" if r["ok"] else "\033[31m✘\033[0m"
                detail = f"{v.get('manager', '?')} {v.get('package', '')} {v.get('installed', '')} running {v.get('running', '') or '-'}"
                print(f"  {mark} {pkg:<11} {detail}  ({r['seconds']}s)" + ("" if r["ok"] else f"\n      {r.get('error')}"), flush=True)
                if not r["ok"] and r.get("output"):
                    tail = [ln for ln in r["output"].splitlines() if ln.strip()][-6:]
                    print("\n".join(f"        | {ln[:160]}" for ln in tail), flush=True)
        finally:
            if "--keep" not in sys.argv:
                sh("docker", "rm", "-f", name, check=False)
    os.makedirs(os.path.join(HERE, ".lab"), exist_ok=True)
    with open(os.path.join(HERE, ".lab", "recipe-test.json"), "w", encoding="utf-8") as f:
        json.dump(results, f, indent=1)
    bad = [r for r in results if not r["ok"]]
    print(f"\n{len(results) - len(bad)}/{len(results)} passed")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
