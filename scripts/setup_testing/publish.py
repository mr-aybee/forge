#!/usr/bin/env python3
"""Builds Forge and publishes it as a new release on the local feed.

Picks a version newer than every earlier local build
(<next Cargo patch version>-dev.<yyyyMMdd.HHmmss>), builds the static Linux binaries
(natively on Linux, in WSL on Windows), packages and signs them like the
GitHub release workflow, and publishes them under
scripts/setup_testing/releases/v<version>/. A running feed_server.py serves
the new release right away; Forge servers pointed at the feed offer it under
"Update to ...".

Examples:
  python3 scripts/setup_testing/publish.py
  python3 scripts/setup_testing/publish.py --ui --notes "Try the new nodes page"
  python3 scripts/setup_testing/publish.py --prune 5
  python3 scripts/setup_testing/publish.py --arm      # also Linux arm64
"""
from __future__ import annotations

import argparse
import datetime as dt
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
HERE = REPO / "scripts" / "setup_testing"
RELEASES = HERE / "releases"
WINDOWS = os.name == "nt"


def die(msg: str, code: int = 1) -> None:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(code)


def call(cmd: list[str], **kw) -> int:
    exe = shutil.which(cmd[0]) or cmd[0]
    return subprocess.call([exe, *cmd[1:]], **kw)


def git(*args: str) -> str:
    p = subprocess.run(["git", "-C", str(REPO), *args], capture_output=True, text=True)
    return p.stdout.strip() if p.returncode == 0 else ""


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--notes", default="", help="release notes shown in the update dialog (default: git summary)")
    ap.add_argument("--ui", action="store_true", help="rebuild the web console first (done anyway when never built)")
    ap.add_argument("--distro", default="Ubuntu-24.04", help="WSL distribution with Rust and the musl target (Windows only)")
    ap.add_argument("--prune", type=int, default=0, help="keep only the newest N releases afterwards (0 = keep all)")
    ap.add_argument("--arm", action="store_true",
                    help="also build the Linux arm64 (aarch64) assets, with cross (needs Docker)")
    ap.add_argument("--full", action="store_true",
                    help="optimise like a real release (thin LTO, opt-level 3); default is a quick lab build")
    a = ap.parse_args()

    m = re.search(r'^version\s*=\s*"([^"]+)"', (REPO / "src" / "forge" / "Cargo.toml").read_text(encoding="utf-8"), re.M)
    if not m:
        die("no version in Cargo.toml")
    # The next patch version: Forge orders X.Y.Z-dev.… below X.Y.Z, so a dev
    # build of the next version sorts above the last release.
    major, minor, patch = (m.group(1).split("-")[0].split(".") + ["0", "0"])[:3]
    version = f"{major}.{minor}.{int(patch) + 1}-dev.{dt.datetime.now():%Y%m%d.%H%M%S}"

    ui = REPO / "src" / "console"
    if a.ui or not (ui / "dist" / "index.html").exists():
        print("==> Building the web console")
        if not (ui / "node_modules").is_dir() and call(["npm", "ci"], cwd=ui):
            die("npm ci failed")
        if call(["npm", "run", "build"], cwd=ui):
            die("UI build failed")

    notes = a.notes
    if not notes:
        head = git("log", "-1", "--format=%h %s")
        dirty = len([l for l in git("status", "--porcelain").splitlines() if l])
        notes = f"Local build of {head}" + (f" plus {dirty} uncommitted change(s)" if dirty else "")
    RELEASES.mkdir(parents=True, exist_ok=True)
    notes_file = RELEASES / f".notes-{version}.txt"
    notes_file.write_text(notes, encoding="utf-8")

    print(f"==> Publishing v{version}")
    quick = "0" if a.full else "1"
    arches = "x86_64 aarch64" if a.arm else "x86_64"
    print("    build: " + ("full (release optimisation)" if a.full else "quick (lab)"))
    rel_notes = f"scripts/setup_testing/releases/.notes-{version}.txt"
    try:
        if WINDOWS:
            rc = call(["wsl", "-d", a.distro, "--cd", str(REPO), "--", "env", f"FORGE_QUICK={quick}", f"FORGE_ARCHES={arches}",
                       "bash", "scripts/setup_testing/build_release.sh", version, rel_notes])
        else:
            rc = call(["bash", "scripts/setup_testing/build_release.sh", version, rel_notes],
                      cwd=REPO, env=dict(os.environ, FORGE_QUICK=quick, FORGE_ARCHES=arches))
        if rc:
            die("build_release.sh failed", rc)
    finally:
        notes_file.unlink(missing_ok=True)

    if a.prune > 0:
        dirs = sorted((d for d in RELEASES.iterdir() if d.is_dir() and d.name.startswith("v")),
                      key=lambda d: d.stat().st_mtime, reverse=True)
        for d in dirs[a.prune:]:
            print(f"    pruning {d.name}")
            shutil.rmtree(d)
    print(f"Done: v{version} is live on the feed.")


if __name__ == "__main__":
    main()
