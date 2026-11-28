#!/usr/bin/env bash
# Starts the Linux lab in WSL: one control plane + three 1 CPU / 1 GB nodes.
#   wsl -d Ubuntu-24.04 -- bash scripts/lab/up.sh
# UI: http://localhost:8090 (from Windows, via WSL localhost forwarding).
set -euo pipefail
cd "$(dirname "$0")"
LAB="$PWD/.lab"
ROOT="$(cd ../.. && pwd)"
NET=forge-lab
NODES=(node-1 node-2 node-3)

[ -x "$LAB/bin/forge-server" ] || { echo "run scripts/lab/build.sh first" >&2; exit 1; }
[ -f "$ROOT/src/console/dist/index.html" ] || echo "warning: src/console/dist missing; build the UI (npm run build) for the web console" >&2

mkdir -p "$LAB/cp-data"
creds="$LAB/credentials.env"
if [ ! -f "$creds" ]; then
  {
    echo "ADMIN_EMAIL=admin@lab.local"
    echo "ADMIN_PASSWORD=$(head -c 18 /dev/urandom | base64 | tr -d '/+=')"
    echo "NODE_USER=forgeops"
    echo "NODE_PASSWORD=$(head -c 12 /dev/urandom | base64 | tr -d '/+=')"
  } > "$creds"
  chmod 600 "$creds"
fi
# shellcheck disable=SC1090
. "$creds"

docker network inspect "$NET" >/dev/null 2>&1 || docker network create "$NET" >/dev/null
docker build -q -t forge-lab-node -f Dockerfile.node . >/dev/null
docker build -q -t forge-lab-cp -f Dockerfile.cp . >/dev/null

cat > "$LAB/forge.toml" <<TOML
data_dir = "/var/lib/forge"
api_listen = "0.0.0.0:8090"
agent_listen = "0.0.0.0:3843"
# Lab only: no authenticator needed (refused for non-loopback public URLs).
dev_skip_mfa = true
public_url = "http://localhost:8090"
node_public_url = "http://forge-cp:8090"
agent_public_url = "https://forge-cp:3843"
agent_hostnames = ["forge-cp", "localhost", "127.0.0.1"]
cookie_secure = false
ui_dir = "/opt/forge/ui"
agent_dist_dir = "/opt/forge/dist"
plugin_runner = "/opt/forge/bin/forge-plugin-runner"
TOML

cp_mounts=(
  -v "$LAB/bin:/opt/forge/bin:ro" -v "$LAB/dist:/opt/forge/dist:ro"
  -v "$ROOT/src/console/dist:/opt/forge/ui:ro"
  -v "$LAB/forge.toml:/etc/forge/forge.toml:ro" -v "$LAB/cp-data:/var/lib/forge"
)
# Older labs ran the control plane on bare debian (no CA bundle): recreate it
# on the lab image. Its state lives in the mounted cp-data directory.
if docker ps -a --format '{{.Names}}' | grep -qx forge-cp   && [ "$(docker inspect -f '{{.Config.Image}}' forge-cp)" != forge-lab-cp ]; then
  docker rm -f forge-cp >/dev/null
fi
if ! docker ps -a --format '{{.Names}}' | grep -qx forge-cp; then
  # Initialise BEFORE the server starts: the server loads role bindings at
  # startup, so an admin created afterwards would have no access until restart.
  if [ ! -f "$LAB/cp-data/forge.db" ]; then
    docker run --rm "${cp_mounts[@]}" -e FORGE_ADMIN_PASSWORD="$ADMIN_PASSWORD" forge-lab-cp \
      /opt/forge/bin/forge-server --config /etc/forge/forge.toml init --email "$ADMIN_EMAIL" >/dev/null
  fi
  docker run -d --name forge-cp --hostname forge-cp --network "$NET" \
    --cpus 1 --memory 1g -p 8090:8090 -p 3843:3843 "${cp_mounts[@]}" \
    forge-lab-cp /opt/forge/bin/forge-server --config /etc/forge/forge.toml serve >/dev/null
else
  docker start forge-cp >/dev/null
fi

i=0
for n in "${NODES[@]}"; do
  i=$((i + 1))
  if ! docker ps -a --format '{{.Names}}' | grep -qx "$n"; then
    # node-1 may run its own Docker engine (App Store "Docker Engine"), which
    # needs a privileged container; the other nodes are plain VMs.
    extra=()
    [ "$n" = node-1 ] && extra=(--privileged --cgroupns=host -v forge-lab-node1-docker:/var/lib/docker)
    docker run -d --name "$n" --hostname "$n" --network "$NET" \
      --cpus 1 --memory 1g "${extra[@]}" \
      -p "222$i:22" -e FORGEOPS_PASSWORD="$NODE_PASSWORD" \
      forge-lab-node >/dev/null
  else
    docker start "$n" >/dev/null
  fi
done

for _ in $(seq 1 40); do curl -sf http://localhost:8090/api/health >/dev/null && break; sleep 0.5; done

# Control node: forge-cp manages itself through its own agent, enrolled over
# loopback so Forge registers it with the role "control" (DECISIONS.md D-016).
AGENT=/opt/forge/dist/forge-agent-linux-x86_64
AGENT_STATE=/var/lib/forge/control-agent
if [ ! -f "$LAB/cp-data/control-agent/identity.json" ]; then
  jar=$(mktemp)
  # Login body with a solved login challenge (proof of work, D-064).
  body=$(ADMIN_EMAIL="$ADMIN_EMAIL" ADMIN_PASSWORD="$ADMIN_PASSWORD" python3 -c '
import hashlib, json, os, urllib.parse, urllib.request
login = os.environ["ADMIN_EMAIL"]
b = {"login": login, "password": os.environ["ADMIN_PASSWORD"]}
c = json.load(urllib.request.urlopen("http://localhost:8090/api/v1/auth/challenge?login=" + urllib.parse.quote(login)))
if c.get("required"):
    salt, bits, n = bytes.fromhex(c["salt"]), int(c["difficulty"]), 0
    while int.from_bytes(hashlib.sha256(salt + str(n).encode()).digest(), "big") >> (256 - bits):
        n += 1
    b.update(challenge_id=c["id"], nonce=str(n))
print(json.dumps(b))')
  login=$(printf '%s' "$body" | curl -sf -c "$jar" -H 'Content-Type: application/json' --data-binary @- \
    http://localhost:8090/api/v1/auth/login)
  csrf=$(echo "$login" | sed -n 's/.*"csrf_token":"\([^"]*\)".*/\1/p')
  token=$(curl -sf -b "$jar" -H "X-CSRF-Token: $csrf" -H 'Content-Type: application/json' \
    -d '{"ttl_minutes":10}' http://localhost:8090/api/v1/enrollment-tokens | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
  rm -f "$jar"
  docker exec -e FORGE_ENROLL_TOKEN="$token" forge-cp \
    "$AGENT" --state-dir "$AGENT_STATE" enroll --server https://127.0.0.1:3843 --hostname forge-cp >/dev/null
fi
# The control agent only runs read-only commands (including peer probes).
CONF="$LAB/cp-data/control-agent/agent.toml"
grep -q '"probe"' "$CONF" 2>/dev/null || sed -i 's/^\( *\)"service_status",$/\1"service_status",\n\1"probe",/' "$CONF"
docker exec -d forge-cp sh -c "while true; do $AGENT --state-dir $AGENT_STATE run; sleep 2; done"

cat <<EOF

Orbyn Forge lab is up.
  Console:   http://localhost:8090      admin: $ADMIN_EMAIL / $ADMIN_PASSWORD
  Nodes:     ${NODES[*]}  (1 CPU, 1 GB each)
  Node SSH:  user $NODE_USER / $NODE_PASSWORD  (sudo needs the same password; root login disabled)
             From Forge: host node-1 port 22 (lab network).  From WSL: ssh -p 2221 $NODE_USER@localhost
  Credentials are stored in scripts/lab/.lab/credentials.env (git-ignored).
EOF
