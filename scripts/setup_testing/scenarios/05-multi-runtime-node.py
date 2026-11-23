#!/usr/bin/env python3
"""05 · One node, many runtimes, one domain each (see the .md).

Python and PHP services (and .NET with --with-dotnet) on lab-c, each behind
its own endpoint on lab-a, each running as its own Linux account.
"""

from __future__ import annotations

import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from labkit.lab import Lab, parser, run  # noqa: E402

ENV = "s05"


def main() -> int:
    p = parser(__doc__)
    p.add_argument("--with-dotnet", action="store_true")
    a = p.parse_args()
    lab = Lab(a, "05-multi-runtime-node")
    lab.environment(ENV)
    edge = [lab.node_id("lab-a")]
    lab.check("05", "nginx on lab-a", lab.install_package("nginx", "stable", ["lab-a"]))

    apps = [
        ("py", 8001, {"package": "python", "version": "system", "params": {}},
         "python3 -m http.server 8001 --bind 0.0.0.0 --directory /srv/forge/apps/s05/py/site",
         lab.site_zip("py", "<p>s05 python</p>")),
        ("php", 8003, {"package": "php", "version": "8.3", "params": {}},
         "php -S 0.0.0.0:8003 -t /srv/forge/apps/s05/php/site",
         lab.site_zip("php", "", {"index.php": "<?php echo 's05 php ' . PHP_VERSION;"})),
    ]
    for name, port, dep, cmd, archive in apps:
        lab.step(f"{name} on lab-c")
        ok = lab.install_package(dep["package"], dep["version"], ["lab-c"], dep["params"])
        lab.check("05", f"runtime {dep['package']} {dep['version']}", ok)
        app = lab.create_app(ENV, {"name": name, "runtime": "systemd", "listen_port": port, "restart": True,
                                   "env": [], "command": cmd, "dependencies": [dep]})
        r = lab.place_and_deploy(name, ENV, ["lab-c"])
        lab.check("05", f"{name} deployed", r.ok, r.message)
        lab.upload_and_extract(name, ENV, archive, "/site")
        lab.forge("services", "action", name, "restart", "--node", "lab-c", "--environment", ENV, "--yes")
        s = lab.site({"name": f"s05-{name}", "domains": [f"{name}.s05.lab.local"], "upstreams": [], "tls": "off",
                      "balance": "round_robin", "edges": edge, "targets": [{"kind": "app", "service_id": app["id"], "port": port}]})
        lab.check("05", f"endpoint for {name}", s.ok, s.message)

    time.sleep(3)
    for name, *_ in apps:
        code, body = lab.http(f"{name}.s05.lab.local")
        lab.check("05", f"{name}.s05.lab.local answers 200 with its own content", code == 200 and f"s05 {name if name != 'py' else 'python'}" in body,
                  f"{code} {body[:40]}")
    ps = lab.node_sh("lab-c", "ps -eo user=,args= | grep -E 'http.server 8001|php -S' | grep -v grep | awk '{print $1}' | sort -u")
    users = str(ps.result).split()
    lab.check("05", "each app runs as its own account, never root", len(users) == 2 and "root" not in users, str(users))
    deny = lab.node_sh("lab-c", "runuser -u s05-py -- ls /srv/forge/apps/s05/php")
    lab.check("05", "one app cannot read another's folder", not deny.ok and "denied" in deny.message.lower(), deny.message[:80])

    lab.step("change one app; the others are unaffected")
    # No typed flag for limits: change the stored definition through the API.
    cur = lab.service("py", ENV)
    spec = {k: v for k, v in (cur or {}).get("spec", {}).items() if k not in ("environment", "health")}
    body = dict(spec, runtime="systemd", memory_mb=64,
                env=[{"key": e["key"], "value": e.get("value"), "secret": e.get("secret", False)} for e in (cur or {}).get("env", [])])
    up = lab.api("PUT", f"/api/v1/services/{(cur or {}).get('id')}", body)
    lab.check("05", "memory limit saved", up.ok, up.message)
    lab.forge("services", "deploy", "py", "--environment", ENV, "--wait", "--yes", timeout=900)
    mem = lab.node_sh("lab-c", "systemctl show forge-app-s05-py -p MemoryMax --value")
    lab.check("05", "memory limit applied to py only", str(mem.result).strip() == str(64 * 1024 * 1024), str(mem.result))
    lab.check("05", "php still answers", lab.http("php.s05.lab.local")[0] == 200)

    if not a.keep:
        lab.step("clean up")
        for name, *_ in apps:
            lab.drop_endpoint(f"s05-{name}")
            lab.drop_service(name, ENV)
    return lab.finish()


if __name__ == "__main__":
    run(main)
