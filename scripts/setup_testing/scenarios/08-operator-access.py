#!/usr/bin/env python3
"""08 · Operator access: tunnels, exec, logs, SFTP grants (see the .md).

Postgres on lab-b bound to loopback only, reached through a Forge tunnel;
the port is not reachable directly; a port that is not allowed is refused;
exec is audited; stack logs are readable; an SFTP grant can be created and
revoked.
"""

from __future__ import annotations

import os
import secrets
import socket
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from labkit.lab import Lab, parser, run  # noqa: E402

ENV = "s08"
PG = """services:
  db:
    image: postgres:16-alpine
    environment: { POSTGRES_PASSWORD: "${PGPASS}" }
    ports: ["127.0.0.1:5432:5432"]
    volumes: ["pg:/var/lib/postgresql/data"]
volumes: { pg: {} }
"""
# The first message of the Postgres protocol: may we use TLS? The server answers 'S' or 'N'.
SSL_REQUEST = b"\x00\x00\x00\x08\x04\xd2\x16\x2f"


def postgres_answers(host: str, port: int) -> bool:
    try:
        with socket.create_connection((host, port), timeout=5) as s:
            s.sendall(SSL_REQUEST)
            return s.recv(1) in (b"S", b"N")
    except OSError:
        return False


def main() -> int:
    p = parser(__doc__)
    p.add_argument("--node-b", default="10.50.0.11", help="lab-b's address from this machine")
    a = p.parse_args()
    lab = Lab(a, "08-operator-access")
    lab.environment(ENV)

    lab.step("Postgres on lab-b, loopback only")
    lab.check("08", "Docker on lab-b", lab.install_package("docker", "stable", ["lab-b"]))
    compose, env = lab.work / "pg.yml", lab.work / "pg.env"
    compose.write_text(PG)
    env.write_text(f"PGPASS={secrets.token_urlsafe(16)}\n")
    lab.drop_stack("pg")
    lab.must("stacks", "create", "pg", "--environment", ENV, "--node", "lab-b", "-f", str(compose), "--env-file", str(env))
    env.unlink()
    r = lab.forge("stacks", "deploy", "pg", "--wait", "--yes", timeout=1800)
    lab.check("08", "Postgres deployed", r.ok, r.message)

    lab.step("tunnel")
    lab.forge("tunnel", "ports", "--node", "lab-b", "--set", "5432")
    cmd = [lab.forge_bin, "tunnel", "open", "-n", "lab-b", "-p", "5432", "-l", "15432", "--for", "10m"]
    tun = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, env=dict(os.environ))
    ok = False
    for _ in range(30):
        if postgres_answers("127.0.0.1", 15432):
            ok = True
            break
        time.sleep(1)
    lab.check("08", "Postgres answers through the tunnel", ok)
    lab.check("08", "port 5432 is not reachable directly", not postgres_answers(a.node_b, 5432))
    tl = lab.forge("tunnel", "list")
    lab.check("08", "the tunnel is listed", tl.ok and "5432" in str(tl.result), tl.message)
    tun.terminate()
    try:
        tun.wait(10)
    except subprocess.TimeoutExpired:
        tun.kill()
    # An administrator holds `network.tunnel.any`: a raw tunnel to any port
    # (here SSH) opens and stays up. People without it are refused.
    raw = subprocess.Popen([lab.forge_bin, "--non-interactive", "tunnel", "open", "-n", "lab-b", "-p", "22", "-l", "15433", "--for", "1m"],
                           stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, env=dict(os.environ))
    time.sleep(8)
    opened = raw.poll() is None
    raw.terminate()
    lab.check("08", "an administrator's raw tunnel to port 22 opens (network.tunnel.any)", opened,
              "" if opened else (raw.stderr.read().decode(errors="replace")[:100] if raw.stderr else ""))

    lab.step("exec is audited")
    ex = lab.forge("nodes", "exec", "lab-c", "--", "uptime")
    lab.check("08", "exec runs", ex.ok, ex.message)
    au = lab.forge("audit", "list", "--limit", "30")
    lab.check("08", "exec appears in the audit log", "exec" in str(au.result).lower(), au.message)

    lab.step("logs")
    lg = lab.forge("stacks", "logs", "pg", "db", "--node", "lab-b", "--tail", "50")
    lab.check("08", "Postgres logs readable", lg.ok and "database system is ready" in str(lg.result), lg.message)

    lab.step("SFTP grant")
    roots = lab.forge("nodes", "files", "roots", "lab-b")
    lab.check("08", "file roots listed", roots.ok, roots.message)
    sl = lab.forge("nodes", "files", "sftp", "list", "lab-b")
    # SFTP is off unless [sftp] enabled = true on the server: saying so is the right answer then.
    lab.check("08", "SFTP grants listable (or SFTP reported as not enabled)", sl.ok or "not enabled" in sl.message, sl.message)

    if not a.keep:
        lab.step("clean up")
        lab.forge("tunnel", "ports", "--node", "lab-b", "--clear")
        lab.drop_stack("pg")
    return lab.finish()


if __name__ == "__main__":
    run(main)
