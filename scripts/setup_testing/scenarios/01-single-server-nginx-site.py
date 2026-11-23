#!/usr/bin/env python3
"""01 · One server hosts everything (see the .md).

Server, agent, a native website (bound to 127.0.0.1) and nginx all on lab-a.
"""

from __future__ import annotations

import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from labkit.lab import Lab, parser, run  # noqa: E402

ENV, DOMAIN = "s01", "s01.lab.local"


def main() -> int:
    a = parser(__doc__).parse_args()
    lab = Lab(a, "01-single-server-nginx-site")
    lab.environment(ENV)

    lab.step("nginx on lab-a")
    lab.check("01", "nginx installed on lab-a", lab.install_package("nginx", "stable", ["lab-a"]))
    r = lab.node_sh("lab-a", "systemctl is-active nginx")
    lab.check("01", "nginx active", r.ok, r.message)
    lab.check("01", "the Forge console still answers on :8080", lab.server_ok())

    lab.step("the website as a native service on 127.0.0.1:8081")
    lab.create_app(ENV, {
        "name": "web", "runtime": "systemd", "listen_port": 8081, "restart": True, "env": [],
        "command": "python3 -m http.server 8081 --bind 127.0.0.1 --directory /srv/forge/apps/s01/web/site",
        "dependencies": [{"package": "python", "version": "system", "params": {}}],
    })
    r = lab.place_and_deploy("web", ENV, ["lab-a"])
    lab.check("01", "deployed on lab-a", r.ok, r.message)
    up = lab.upload_and_extract("web", ENV, lab.site_zip("site", "<h1>s01 one server</h1>"), "/site")
    lab.check("01", "site uploaded and extracted into the app folder", up.ok, up.message)
    lab.forge("services", "action", "web", "restart", "--node", "lab-a", "--environment", ENV, "--yes")
    time.sleep(2)
    r = lab.node_sh("lab-a", "curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8081/")
    lab.check("01", "the app answers on loopback", str(r.result).strip().endswith("200"), str(r.result))

    lab.step("endpoint on the same node")
    r = lab.site({"name": "s01-web", "domains": [DOMAIN], "upstreams": ["127.0.0.1:8081"], "balance": "round_robin",
                  "tls": "off", "edges": [lab.node_id("lab-a")]})
    lab.check("01", "endpoint applied", r.ok and (r.result or {}).get("edges", [{}])[0].get("result", {}).get("status") != "failed", r.message)
    code, body = lab.http(DOMAIN)
    lab.check("01", "site returns 200 through nginx", code == 200 and "s01 one server" in body, f"{code}")
    show = lab.must("services", "show", "web", "--environment", ENV)
    inst = (show or {}).get("instances", [])  # type: ignore[union-attr]
    lab.check("01", "instance deployed/active", bool(inst) and inst[0].get("status") == "deployed" and inst[0].get("state") == "active", str(inst))

    lab.step("the site survives a server restart")
    lab.node_sh("lab-a", "systemd-run --on-active=1s systemctl restart forge-server")
    time.sleep(3)
    ok = all(lab.http(DOMAIN)[0] == 200 for _ in range(10))
    lab.check("01", "site keeps serving while Forge restarts", ok)
    for _ in range(40):
        if lab.forge("auth", "whoami").ok:
            break
        time.sleep(2)

    if not a.keep:
        lab.step("clean up")
        lab.drop_endpoint("s01-web")
        lab.drop_service("web", ENV)
    return lab.finish()


if __name__ == "__main__":
    run(main)
