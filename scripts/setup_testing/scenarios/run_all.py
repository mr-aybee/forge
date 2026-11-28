#!/usr/bin/env python3
"""Runs the lab scenarios from Windows and from WSL, each from checkpoint `clean`.

    python run_all.py                     # every scenario, Windows then WSL
    python run_all.py --os windows --only 03 --only 12
    python run_all.py --no-reset          # reuse the current lab (quick re-run)

Run on the Hyper-V host from an admin shell (checkpoint restore, firewall).
For every scenario and OS:

1. `labkit/bootstrap.py`: restore `clean`, install the latest release from the
   local feed, onboard the nodes, cluster `lab`, an API key for the scripts;
2. deployment scenarios (11–17): `10-deploy-setup.py` on Windows (it starts
   the mock GitHub there, which the nodes download from);
3. the scenario: on Windows with `forge.exe`, or in WSL with the Linux
   `forge` from the same release, through `labkit/forward.py` (WSL has no
   route to the lab network).

Logs go to `.lab/runs/<time>/<os>-<scenario>.log`; the summary is printed and
written to `summary.md` there. Exit 0 only when everything passed.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
SETUP = HERE.parent
REPO = SETUP.parent.parent
LAB = SETUP / ".lab"
DISTRO = "Ubuntu-24.04"
FORWARDS = ["18080=10.50.0.10:3880", "18081=10.50.0.10:80", "18082=10.50.0.11:80"]

SCENARIOS = sorted(p.name for p in HERE.glob("[0-9][0-9]-*.py") if not p.name.startswith(("00-", "10-")))


def wsl_path(p: Path) -> str:
    s = str(p.resolve()).replace("\\", "/")
    return f"/mnt/{s[0].lower()}{s[2:]}"


def latest_release() -> Path:
    rel = sorted((SETUP / "releases").glob("v*"), key=lambda p: p.stat().st_mtime)
    if not rel:
        raise SystemExit("no release in scripts/setup_testing/releases: run publish.py first")
    return rel[-1]


def run(cmd: list[str], log: Path, timeout: int, env: dict | None = None) -> int:
    with log.open("a", encoding="utf-8") as f:
        f.write(f"\n$ {' '.join(cmd)}\n")
        f.flush()
        try:
            p = subprocess.run(cmd, stdout=f, stderr=subprocess.STDOUT, timeout=timeout, env=env)
            return p.returncode
        except subprocess.TimeoutExpired:
            f.write(f"\n!! timed out after {timeout}s\n")
            return 124


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--os", action="append", choices=["windows", "wsl"], help="default: both")
    ap.add_argument("--only", action="append", default=[], help="scenario number, e.g. 03; repeatable")
    ap.add_argument("--no-reset", action="store_true", help="do not restore `clean` (reuse the lab as it is)")
    ap.add_argument("--forge-exe", default=str(REPO / "src" / "forge" / "target" / "debug" / "forge.exe"), help="the Windows forge CLI")
    ap.add_argument("--timeout", type=int, default=90, help="minutes per scenario")
    a = ap.parse_args()
    systems = a.os or ["windows", "wsl"]
    release = latest_release()
    linux_forge = release / "forge-cli-x86_64-unknown-linux-musl"
    runs = LAB / "runs" / time.strftime("%Y%m%d-%H%M%S")
    runs.mkdir(parents=True, exist_ok=True)
    py = sys.executable
    forwarder = None
    if "wsl" in systems:
        forwarder = subprocess.Popen([py, str(HERE / "labkit" / "forward.py"), *FORWARDS],
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    results: list[tuple[str, str, str, float]] = []
    try:
        for system in systems:
            for name in SCENARIOS:
                if a.only and name[:2] not in a.only:
                    continue
                stem = name[:-3]
                log = runs / f"{system}-{stem}.log"
                print(f"[{time.strftime('%H:%M:%S')}] {system:7} {stem}", flush=True)
                start = time.time()
                boot = [py, str(HERE / "labkit" / "bootstrap.py")] + (["--no-rebuild"] if a.no_reset else [])
                if run(boot, log, 3600) != 0:
                    results.append((system, stem, "LAB FAILED", time.time() - start))
                    continue
                env = dict(os.environ, FORGE_BIN=a.forge_exe)
                if name[:2] >= "11":
                    if run([py, str(HERE / "10-deploy-setup.py"), "--forge", a.forge_exe], log, 3600, env) != 0:
                        results.append((system, stem, "SETUP FAILED", time.time() - start))
                        continue
                if system == "windows":
                    rc = run([py, str(HERE / name), "--forge", a.forge_exe], log, a.timeout * 60, env)
                else:
                    extra = ["--server", "http://127.0.0.1:18080", "--edge", "127.0.0.1:18081"]
                    if name.startswith("07-"):
                        extra += ["--edge-b", "127.0.0.1:18082"]
                    # The script reads the API key from .lab/forge-admin.token (same file
                    # through /mnt): never on a command line.
                    cmd = ["wsl", "-d", DISTRO, "--", "env", "FORGE_ALLOW_HTTP=1",
                           "python3", wsl_path(HERE / name), "--forge", wsl_path(linux_forge), *extra]
                    rc = run(cmd, log, a.timeout * 60)
                results.append((system, stem, "PASS" if rc == 0 else f"FAIL({rc})", time.time() - start))
                print(f"           → {results[-1][2]} in {results[-1][3] / 60:.1f} min", flush=True)
    finally:
        if forwarder:
            forwarder.terminate()
    lines = ["| OS | Scenario | Result | Minutes |", "| --- | --- | --- | --- |"]
    lines += [f"| {s} | {n} | {r} | {t / 60:.1f} |" for s, n, r, t in results]
    (runs / "summary.md").write_text("\n".join(lines) + "\n")
    (runs / "summary.json").write_text(json.dumps(results, indent=2))
    print("\n".join(lines))
    print(f"\nlogs: {runs}")
    sys.exit(0 if results and all(r == "PASS" for _, _, r, _ in results) else 1)


if __name__ == "__main__":
    main()
