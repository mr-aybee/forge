#!/bin/sh
# Starts the Python portal with PostgreSQL support: a venv with psycopg next
# to the sources (VENV overrides), then portal.py. Needs python3-venv.
set -eu
cd "$(dirname "$0")"
VENV="${VENV:-./.venv}"
[ -x "$VENV/bin/python" ] || python3 -m venv "$VENV"
"$VENV/bin/pip" install -q --disable-pip-version-check -r requirements.txt
exec "$VENV/bin/python" -u portal.py
