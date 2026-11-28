#!/usr/bin/env python3
"""Fast Rust build environment for this repository (Linux and Windows).

sccache caches compiled crates outside target/, so they survive `cargo clean`
and are shared with every other Rust project on the machine. Incremental
compilation stays on: sccache covers dependencies, incremental covers the
workspace crates you edit.

A Python process cannot change its parent shell, so turn sccache on in one of
these ways:

  * Once per machine (recommended): `[build] rustc-wrapper = "sccache"` in the
    user-level Cargo config (~/.cargo/config.toml, or
    %USERPROFILE%\\.cargo\\config.toml). Never in the repository's
    .cargo/config.toml: that breaks CI and `cross`.
  * Per terminal:
        bash/zsh:    eval "$(python3 scripts/dev/build_env.py --shell sh)"
        PowerShell:  python scripts/dev/build_env.py --shell pwsh | Out-String | Invoke-Expression

Examples:
  python3 scripts/dev/build_env.py                 # report the cache setup
  python3 scripts/dev/build_env.py --sweep --days 7
"""
from __future__ import annotations

import argparse
import shutil
import subprocess
import sys

from _common import RUST, build_env


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--shell", choices=["sh", "pwsh"], help="print commands that set the variables in your shell")
    ap.add_argument("--sweep", action="store_true", help="delete target/ artifacts unused for --days days (cargo-sweep)")
    ap.add_argument("--days", type=int, default=14)
    a = ap.parse_args()

    env = build_env(quiet=bool(a.shell))
    wrapper = env.get("RUSTC_WRAPPER", "")
    if a.shell:
        if wrapper:
            print(f'$env:RUSTC_WRAPPER = "{wrapper}"' if a.shell == "pwsh" else f"export RUSTC_WRAPPER={wrapper}")
        return

    if shutil.which("sccache"):
        subprocess.call(["sccache", "--show-stats"])

    if a.sweep:
        if shutil.which("cargo-sweep"):
            subprocess.call(["cargo", "sweep", "--time", str(a.days), str(RUST)])
        else:
            print("cargo-sweep not found; install it with: cargo install cargo-sweep --locked")


if __name__ == "__main__":
    sys.exit(main())
