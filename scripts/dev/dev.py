#!/usr/bin/env python3
"""Fast local development: the web console with instant reload (Vite HMR),
against either the lab server or a forge-server running on this machine.

  --mode lab    (default) UI only. Vite on http://localhost:55132 proxies /api
                and /oauth to the lab server (--lab-url). Every save in src/ui
                shows up in the browser at once. Real nodes, real data. New
                backend endpoints only exist after the lab server is updated.

  --mode local  forge-server runs on this machine from the working tree
                (cargo run), with its own data in --data-dir, MFA off,
                environment development, on 127.0.0.1:55130. Vite proxies to
                it. The server runs in the background and rebuilds and restarts
                whenever it exits; its output goes to <data-dir>/server.log.
                Ctrl-C stops both. No nodes unless you enroll one.

The server's own UI (built dist) is not used: browse http://localhost:55132.

Examples:
  python3 scripts/dev/dev.py                 # UI against the lab
  python3 scripts/dev/dev.py --mode local    # local server + UI
"""
from __future__ import annotations

import argparse
import getpass
import os
import subprocess
import sys
import threading
from pathlib import Path

from _common import EXE, REPO, WINDOWS, build_env, cyan, dev_version, die, green, run, tool

DEFAULT_DATA = Path(r"D:\forge-dev") if WINDOWS else Path.home() / ".local" / "share" / "forge-dev"

CONFIG = """# Local development server (scripts/dev/dev.py). Not for production.
data_dir = "{data}"
api_listen = "127.0.0.1:55130"
agent_listen = "127.0.0.1:55131"
public_url = "http://localhost:55132"
environment = "development"
cookie_secure = false

[security]
require_mfa = false

[update]
enabled = false
"""


def server_loop(cfg: Path, log: Path, stop: threading.Event, current: list[subprocess.Popen]) -> None:
    env = build_env(quiet=True)
    # A dev stamp (one per session, so a restart does not force a rebuild):
    # the server then orders correctly against the dev agents in the dist
    # folder and against releases.
    env.setdefault("FORGE_BUILD_VERSION", dev_version())
    with log.open("a", encoding="utf-8") as out:
        while not stop.is_set():
            out.write("--- building and starting forge-server ---\n")
            out.flush()
            proc = subprocess.Popen([tool("cargo"), "run", "-p", "forge-server", "--", "--config", str(cfg), "serve"],
                                    cwd=REPO, env=env, stdout=out, stderr=subprocess.STDOUT)
            current[:] = [proc]
            proc.wait()
            stop.wait(1)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--mode", choices=["lab", "local"], default="lab")
    ap.add_argument("--lab-url", default="http://10.50.0.10:8080")
    ap.add_argument("--data-dir", type=Path, default=DEFAULT_DATA)
    a = ap.parse_args()

    ui = REPO / "src" / "ui"
    if not (ui / "node_modules").is_dir():
        run(["npm", "ci"], cwd=ui)

    env = dict(os.environ)
    stop = threading.Event()
    current: list[subprocess.Popen] = []
    if a.mode == "local":
        data_dir: Path = a.data_dir.resolve()
        data_dir.mkdir(parents=True, exist_ok=True)
        cfg = data_dir / "forge.toml"
        if not cfg.exists():
            cfg.write_text(CONFIG.format(data=(data_dir / "data").as_posix()), encoding="utf-8")
            print(f"wrote {cfg}")

        # First run: build once and create the administrator.
        if not (data_dir / "data" / "forge.db").exists():
            run(["cargo", "build", "-p", "forge-server"], cwd=REPO, env=build_env(), what="build")
            pw = getpass.getpass("Choose a password for the local admin (12+ characters): ")
            init_env = dict(os.environ, FORGE_ADMIN_PASSWORD=pw)
            server = REPO / "target" / "debug" / f"forge-server{EXE}"
            rc = subprocess.call([str(server), "--config", str(cfg), "init", "--username", "admin"], env=init_env)
            if rc:
                die("forge-server init failed", rc)

        log = data_dir / "server.log"
        threading.Thread(target=server_loop, args=(cfg, log, stop, current), daemon=True).start()
        env["FORGE_API"] = "http://127.0.0.1:55130"
        print(f"forge-server builds and starts in the background (first build takes a few minutes); log: {log}")
    else:
        env["FORGE_API"] = a.lab_url

    print(green(f"UI: http://localhost:55132  ->  API {env['FORGE_API']}"))
    try:
        subprocess.call([tool("npm"), "run", "dev"], cwd=ui, env=env)
    except KeyboardInterrupt:
        pass
    finally:
        stop.set()
        for proc in current:
            if proc.poll() is None:
                proc.terminate()


if __name__ == "__main__":
    main()
