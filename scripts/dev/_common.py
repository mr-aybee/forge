"""Shared helpers for the developer scripts in scripts/dev (Linux and Windows).

Standard library only. Imported by the scripts next to it; not a CLI.
"""
from __future__ import annotations

import datetime as dt
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

WINDOWS = os.name == "nt"
EXE = ".exe" if WINDOWS else ""
REPO = Path(__file__).resolve().parents[2]


def die(msg: str, code: int = 1) -> None:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(code)


def cyan(text: str) -> str:
    return f"\033[36m{text}\033[0m" if sys.stdout.isatty() else text


def green(text: str) -> str:
    return f"\033[32m{text}\033[0m" if sys.stdout.isatty() else text


def tool(name: str) -> str:
    """Resolves a tool on PATH (npm is npm.cmd on Windows)."""
    found = shutil.which(name)
    if not found:
        die(f"{name} not found on PATH")
    return found  # type: ignore[return-value]


def run(cmd: list[str], *, cwd: Path | None = None, env: dict[str, str] | None = None, what: str = "") -> None:
    """Runs a command with inherited stdio and stops the script when it fails."""
    rc = subprocess.call([tool(cmd[0]), *cmd[1:]], cwd=cwd, env=env)
    if rc != 0:
        die(f"{what or cmd[0]} failed (exit {rc})", rc)


def build_env(quiet: bool = False) -> dict[str, str]:
    """os.environ plus sccache as RUSTC_WRAPPER when sccache is installed.

    Leaves RUSTC_WRAPPER and SCCACHE_DIR alone when they are already set, so a
    machine-wide setup (~/.cargo/config.toml, ~/.config/sccache/config) wins.
    The cache is shared by every Rust project on the machine.
    """
    env = dict(os.environ)
    if env.get("RUSTC_WRAPPER"):
        return env
    if shutil.which("sccache"):
        env["RUSTC_WRAPPER"] = "sccache"
        if not quiet:
            where = env.get("SCCACHE_DIR", "sccache's default cache directory")
            print(f"sccache on, cache at {where}")
    elif not quiet:
        print("sccache not found; install it with: cargo install sccache --locked")
    return env


def cargo_version() -> str:
    """The workspace version in Cargo.toml (`0.1.0`)."""
    m = re.search(r'^version\s*=\s*"([^"]+)"', (REPO / "Cargo.toml").read_text(encoding="utf-8"), re.M)
    if not m:
        die("no version in Cargo.toml")
    return m.group(1)


def dev_version() -> str:
    """A dev build stamp: `<next patch version>-dev.<yyyyMMdd.HHmmss>`.

    Forge orders `X.Y.Z-dev.…` below `X.Y.Z` (forge_sdk::release::compare_release),
    so a dev build of the next version sorts above the last release and below
    the next one. Use one stamp for the server and the agents of one build.
    """
    major, minor, patch = (cargo_version().split("-")[0].split(".") + ["0", "0"])[:3]
    return f"{major}.{minor}.{int(patch) + 1}-dev.{dt.datetime.now():%Y%m%d.%H%M%S}"


def read_env_file(path: Path) -> dict[str, str]:
    """KEY=value lines; quotes around the value are dropped."""
    out: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            k, v = line.split("=", 1)
            out[k.strip()] = v.strip().strip("'\"")
    return out
