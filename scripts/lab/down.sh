#!/usr/bin/env bash
# Stops the lab. `--reset` also deletes containers and all lab data.
set -euo pipefail
cd "$(dirname "$0")"
names=(forge-cp node-1 node-2 node-3)
if [ "${1:-}" = "--reset" ]; then
  docker rm -f "${names[@]}" >/dev/null 2>&1 || true
  docker network rm forge-lab >/dev/null 2>&1 || true
  rm -rf .lab/cp-data .lab/credentials.env .lab/forge.toml
  echo "lab removed (binaries in .lab/bin and .lab/dist kept)"
else
  docker stop "${names[@]}" >/dev/null 2>&1 || true
  echo "lab stopped; run up.sh to start it again"
fi
