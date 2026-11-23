#!/usr/bin/env python3
"""Runs the fleet-sim benchmark against SQLite and, optionally, Postgres.

Builds forge-server (with the postgres feature), forge-agent and fleet-sim
with the perf profile unless --no-build, then runs fleet-sim for each fleet
size on SQLite and, when --postgres-url is given, on Postgres. Every Postgres
run gets a new, empty database named --postgres-db, created and dropped
through psql; that database is dropped again at the end. Nothing else on the
Postgres server is touched, and no running Forge instance is used: fleet-sim
starts its own server on free ports with a temporary data directory.

Results go to --out-dir: one JSON file per run and summary.md with the tables.

--postgres-url is the URL of the admin connection, without a database name,
for example postgres://<user>:<password>@localhost:5432. psql reads the
password from it; it is never written to the output.

Examples:
  python3 scripts/dev/bench.py
  python3 scripts/dev/bench.py --postgres-url postgres://<user>:<password>@localhost:5432 --agents 50 1000
  python3 scripts/dev/bench.py --postgres-url postgres://<user>:<password>@localhost:5432 --no-names --no-build
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
from urllib.parse import unquote, urlsplit

from _common import EXE, REPO, WINDOWS, build_env, die, run


def default_psql() -> str:
    if shutil.which("psql"):
        return "psql"
    if WINDOWS:
        found = sorted(Path(r"C:\Program Files\PostgreSQL").glob(r"*\bin\psql.exe"))
        if found:
            return str(found[-1])
    return "psql"


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--agents", type=int, nargs="+", default=[50, 1000])
    ap.add_argument("--postgres-url")
    ap.add_argument("--postgres-db", default="forge_bench")
    ap.add_argument("--psql", default=default_psql())
    ap.add_argument("--out-dir", type=Path, default=REPO / "target" / "bench")
    ap.add_argument("--no-names", action="store_true")
    ap.add_argument("--skip-sqlite", action="store_true")
    ap.add_argument("--no-build", action="store_true")
    a = ap.parse_args()

    if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", a.postgres_db):
        die("--postgres-db must be a plain identifier")
    out_dir: Path = a.out_dir.resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    sim = REPO / "target" / "perf" / f"fleet-sim{EXE}"

    if not a.no_build:
        run(["cargo", "build", "--profile", "perf", "-p", "forge-server", "--features", "postgres",
             "-p", "forge-agent", "-p", "fleet-sim"], cwd=REPO, env=build_env(), what="build")

    common = ["--no-agent-process"] + (["--no-names"] if a.no_names else [])
    tag = "no-names" if a.no_names else "names"
    summary = [f"# fleet-sim {dt.datetime.now():%Y-%m-%d %H:%M} ({tag})", ""]

    def sim_run(label: str, sizes: list[int], db_url: str | None) -> list[str]:
        out = out_dir / f"{label}.json"
        args = [str(sim)]
        for n in sizes:
            args += ["--agents", str(n)]
        env = dict(os.environ)
        env.pop("FORGE_DATABASE_URL", None)
        if db_url:
            env["FORGE_DATABASE_URL"] = db_url
        p = subprocess.run(args + common + ["--out", str(out)], env=env, text=True,
                           stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        if p.returncode != 0:
            die(f"fleet-sim failed ({label}): {p.stdout}")
        return [l for l in p.stdout.splitlines() if l.startswith("|")]

    def psql(sql: str) -> str:
        u = urlsplit(a.postgres_url)
        env = dict(os.environ)
        env["PGPASSWORD"] = unquote(u.password or "")
        p = subprocess.run([a.psql, "-U", unquote(u.username or ""), "-h", u.hostname or "localhost",
                            "-p", str(u.port or 5432), "-d", "postgres", "-qtAc", sql],
                           env=env, text=True, capture_output=True)
        if p.returncode != 0:
            die(f"psql failed: {sql}")
        return p.stdout.strip()

    if not a.skip_sqlite:
        print("== SQLite")
        rows = sim_run(f"sqlite-{tag}", a.agents, None)
        summary += ["## SQLite", "", *rows, ""]

    if a.postgres_url:
        summary += ["## PostgreSQL", ""]
        header = True
        db = a.postgres_db
        try:
            for n in a.agents:
                print(f"== Postgres, {n} agents")
                psql(f"DROP DATABASE IF EXISTS {db}")
                psql(f"CREATE DATABASE {db}")
                rows = sim_run(f"postgres-{n}-{tag}", [n], f"{a.postgres_url.rstrip('/')}/{db}")
                size = psql(f"SELECT round(pg_database_size('{db}')/1048576.0, 2)")
                if header:
                    summary += rows[:2]
                    header = False
                summary += rows[2:]
                summary.append(f"<!-- pg_database_size after {n} agents: {size} MB "
                               "(fleet-sim's DB column reads the SQLite file only) -->")
        finally:
            psql(f"DROP DATABASE IF EXISTS {db}")
        summary.append("")

    text = "\n".join(summary) + "\n"
    (out_dir / "summary.md").write_text(text, encoding="utf-8")
    print(text)
    print(f"Results: {out_dir}")


if __name__ == "__main__":
    main()
