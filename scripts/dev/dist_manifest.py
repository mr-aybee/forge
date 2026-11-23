#!/usr/bin/env python3
"""Writes <dist>/MANIFEST.json for the agent binaries in a dist folder.

The control plane hands nodes the agents in its `agent_dist_dir` and offers
them as the version in MANIFEST.json (not its own version). Run this after
copying freshly built agents into the folder, with the version they were
built with (FORGE_BUILD_VERSION):

  python3 scripts/dev/dist_manifest.py ~/.local/share/forge-dev/dist 0.1.1-dev.20261122.162457

Restart forge-server afterwards: it reads the folder at start.
"""
from __future__ import annotations

import hashlib
import json
import os
import sys
from pathlib import Path

PREFIX = "forge-agent"


def write_manifest(dist: Path, version: str) -> Path:
    files = {}
    for f in sorted(dist.iterdir()):
        if f.is_file() and f.name.startswith(PREFIX) and not f.name.endswith((".prev", ".new")):
            files[f.name] = hashlib.sha256(f.read_bytes()).hexdigest()
    if not files:
        raise SystemExit(f"no {PREFIX}* binaries in {dist}")
    out = dist / "MANIFEST.json"
    tmp = dist / "MANIFEST.json.tmp"
    tmp.write_text(json.dumps({"version": version, "files": files}, indent=2) + "\n", encoding="utf-8")
    os.replace(tmp, out)
    return out


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    out = write_manifest(Path(sys.argv[1]), sys.argv[2].strip())
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
