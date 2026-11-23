#!/usr/bin/env bash
# Creates and starts a fresh benchmark server in $B (data, config, admin).
# Usage: setup.sh <engine binary> <sha256> [cpu_quota_vcpu] [memory_mb] [max_uploads]
# Environment: B (new directory), FORGE (forge-server binary; the worker
#   binary must sit next to it), ENGINE (`embedded`, the default, or
#   `duckdb`: no engine binary; pass `-` `-` for the first two arguments,
#   and memory_mb sets duckdb_memory_mb).
set -euo pipefail
CH=$1 SUM=$2 CAP=${3:-2} MEM=${4:-2048} UPLOADS=${5:-8} ENGINE=${ENGINE:-embedded}
mkdir -p "$B"
cat > "$B/forge.toml" <<TOML
# Capacity benchmark server (production profile, local, plain HTTP).
data_dir = "$B/data"
api_listen = "127.0.0.1:56130"
agent_listen = "127.0.0.1:56131"
public_url = "http://127.0.0.1:56130"
agent_public_url = "https://127.0.0.1:56131"
agent_hostnames = ["127.0.0.1", "localhost"]
environment = "production"
cookie_secure = false

[security]
require_mfa = false

[update]
enabled = false

[analytics]
engine = "$ENGINE"
embedded_port = 18124
embedded_memory_mb = $MEM
duckdb_memory_mb = $MEM
cpu_quota_vcpu = $CAP
worker_agent_listen = "127.0.0.1:56132"
worker_agent_url = "https://127.0.0.1:56132"
worker_max_uploads = $UPLOADS
TOML
(umask 077; python3 -c "import secrets;print(secrets.token_urlsafe(18))" > "$B/admin-password.txt")
FORGE_ADMIN_PASSWORD=$(cat "$B/admin-password.txt") "$FORGE" --config "$B/forge.toml" init --username admin
[ "$ENGINE" = duckdb ] || "$FORGE" --config "$B/forge.toml" analytics install --binary "$CH" --sha256 "$SUM"
start() {
  (MALLOC_ARENA_MAX=2 setsid nohup "$FORGE" --config "$B/forge.toml" serve >> "$B/server.log" 2>&1 < /dev/null &)
  until curl -sf -o /dev/null http://127.0.0.1:56130/api/health; do sleep 1; done
  echo "server up"
}
start
