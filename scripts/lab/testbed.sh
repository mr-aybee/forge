#!/usr/bin/env bash
# Ingress testbed: lab + mock portals + Nginx endpoints, reachable from the
# Windows browser. Re-runnable; every step reuses what already exists.
#
#   wsl -d Ubuntu-24.04 -- bash scripts/lab/testbed.sh            # standalone (one Nginx)
#   wsl -d Ubuntu-24.04 -- bash scripts/lab/testbed.sh ha         # + second Nginx, PowerDNS, DNS failover
#   wsl -d Ubuntu-24.04 -- bash scripts/lab/testbed.sh failover   # run the link-down test
#   wsl -d Ubuntu-24.04 -- bash scripts/lab/testbed.sh proxy      # only (re)create the browser forward
#   wsl -d Ubuntu-24.04 -- bash scripts/lab/testbed.sh fresh      # rebuild binaries, reset the lab, then standalone
#
# Browser: add to C:\Windows\System32\drivers\etc\hosts (as administrator)
#   127.0.0.1  py.lab.local php.lab.local net.lab.local
# then open http://net.lab.local:8088 (also py./php.). Refresh to see the
# serving node alternate.
set -euo pipefail
cd "$(dirname "$0")"

PORT="${TESTBED_PORT:-8088}"   # Windows port forwarded to the edge
EDGE="${TESTBED_EDGE:-node-1}" # Nginx edge the browser goes through
NET=forge-lab

step() { printf '\n\033[1m== %s\033[0m\n' "$*"; }

proxy() {
  step "Browser forward: localhost:$PORT -> $EDGE:80"
  docker rm -f lab-edge-proxy >/dev/null 2>&1 || true
  docker run -d --name lab-edge-proxy --network "$NET" -p "$PORT:80" --restart unless-stopped \
    alpine/socat "tcp-listen:80,fork,reuseaddr" "tcp-connect:$EDGE:80" >/dev/null
  sleep 1
  for d in py php net; do
    if out=$(curl -fsS -m 5 -H "Host: $d.lab.local" "http://localhost:$PORT/?format=json" 2>/dev/null); then
      node=$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["node"])')
      echo "  ok  http://$d.lab.local:$PORT  (served by $node)"
    else
      echo "  --  http://$d.lab.local:$PORT  not answering yet"
    fi
  done
  cat <<EOF

Add to C:\\Windows\\System32\\drivers\\etc\\hosts (as administrator):
  127.0.0.1  py.lab.local php.lab.local net.lab.local
Console: http://localhost:8090  (login in scripts/lab/.lab/credentials.env)
EOF
}

ensure_lab() {
  if ! docker ps --format '{{.Names}}' | grep -qx forge-cp; then
    step "Starting the lab"
    bash up.sh
  fi
  step "Seeding clusters, nodes and services"
  python3 seed.py
}

case "${1:-standalone}" in
  fresh)
    step "Building binaries and UI"
    bash build.sh
    (cd ../../src/ui && npm run build >/dev/null)
    step "Resetting the lab"
    bash down.sh --reset || true
    bash up.sh
    sleep 5
    python3 seed.py
    python3 -u scenarios.py portals
    python3 -u scenarios.py standalone
    proxy
    ;;
  standalone)
    ensure_lab
    python3 -u scenarios.py portals
    python3 -u scenarios.py standalone
    proxy
    ;;
  ha)
    ensure_lab
    python3 -u scenarios.py portals
    python3 -u scenarios.py ha
    proxy
    ;;
  failover)
    python3 -u scenarios.py failover
    ;;
  proxy)
    proxy
    ;;
  *)
    sed -n '2,16p' "$0"
    exit 2
    ;;
esac
