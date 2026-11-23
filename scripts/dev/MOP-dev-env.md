# MOP: local Forge dev environment (Linux)

Method of procedure for the local environment: a Forge server built from
the working tree, the embedded analytics engine, three disposable Docker nodes,
and two sample portals (Python and .NET) deployed through Forge, with an
Nginx ingress and PostgreSQL.

```
 host (Linux)
 ├─ forge-server (target/debug)      :55130 API + console, :55131 agents
 │    data: ~/.local/share/forge-dev
 │    └─ embedded analytics engine     127.0.0.1:18123 (child process; metrics, logs)
 └─ Docker engine
      └─ network forge-dev 172.30.0.0/24
           ├─ dev-node-1 172.30.0.11  py-portal :8101, net-portal :8103
           ├─ dev-node-2 172.30.0.12  py-portal :8101, net-portal :8103
           └─ dev-node-3 172.30.0.13  PostgreSQL, Nginx ingress
```

The nodes mount the host's Docker socket, so containers they deploy run on
the host engine. Anyone who controls a node controls the host's Docker. Use
this setup only on a development machine.

Throughout, `D=~/.local/share/forge-dev` and commands run from the
repository root unless stated.

## 1. Prerequisites

| Item | Check |
| --- | --- |
| Rust stable + `x86_64-unknown-linux-musl` target | `rustup target add x86_64-unknown-linux-musl` |
| `gcc` (or `musl-tools`) | `gcc --version` |
| Node.js + npm | `npm --version` |
| Docker Engine with Compose v2; your user in the `docker` group | `docker compose version` (use `sg docker -c "..."` if the group is not active in this login yet) |
| Python 3 (standard library only) | `python3 --version` |

Analytics runs in the embedded engine: `forge-server` starts the ClickHouse
engine binary as a supervised child process on `127.0.0.1:18123`, with its
data under `$D/data/analytics`. No ClickHouse server on the host is needed.
The binary is a separate download; step 4 installs it.

## 2. Build the server and the console

```sh
# One dev stamp for the server and the agents of this build: the next patch
# version, so it sorts above the last release (Forge orders X.Y.Z-dev.… below X.Y.Z).
export FORGE_BUILD_VERSION="0.1.1-dev.$(date +%Y%m%d.%H%M%S)"
cargo build -p forge-server
(cd src/ui && npm ci && npm run build)       # served by forge-server from src/ui/dist
```

## 3. Build the agent for the nodes

The server hands the agent binaries to nodes from `agent_dist_dir`. A
quick musl release build (no LTO) is enough:

```sh
mkdir -p $D/dist
export FORGE_BUILD_VERSION="${FORGE_BUILD_VERSION:-0.1.1-dev.$(date +%Y%m%d.%H%M%S)}" \
       CARGO_TARGET_DIR="$HOME/.cache/forge-target-quick" \
       CARGO_PROFILE_RELEASE_OPT_LEVEL=1 CARGO_PROFILE_RELEASE_LTO=off \
       CARGO_PROFILE_RELEASE_CODEGEN_UNITS=256 CARGO_PROFILE_RELEASE_INCREMENTAL=true \
       CC_x86_64_unknown_linux_musl=gcc
cargo build --release --target x86_64-unknown-linux-musl -p forge-agent
T=$CARGO_TARGET_DIR/x86_64-unknown-linux-musl/release
cp $T/forge-agent        $D/dist/forge-agent-linux-x86_64
cp $T/forge-agent-helper $D/dist/forge-agent-helper-linux-x86_64
python3 scripts/dev/dist_manifest.py $D/dist "$FORGE_BUILD_VERSION"   # the version the server offers
echo "$FORGE_BUILD_VERSION" > $D/version.txt
```

Rebuild, copy and write the manifest again whenever agent code changes, then
restart the server; nodes update from it. The server offers the agents as the
version in `MANIFEST.json` and skips any file whose hash does not match it.

## 4. Configure the server

Write `$D/forge.toml`. Replace `<host-ip>` with the address you open the
console from (for example a VPN or LAN address). `172.17.0.1` is the Docker
bridge gateway, which is how containers reach the host.

```toml
# Local development server. Not for production.
data_dir = "/home/<user>/.local/share/forge-dev/data"
api_listen = "0.0.0.0:55130"
agent_listen = "0.0.0.0:55131"
public_url = "http://<host-ip>:55130"
node_public_url = "http://172.17.0.1:55130"
agent_public_url = "https://172.17.0.1:55131"
agent_hostnames = ["172.17.0.1", "localhost", "127.0.0.1"]
agent_dist_dir = "/home/<user>/.local/share/forge-dev/dist"
ui_dir = "/home/<user>/repos/Forge/src/ui/dist"
environment = "development"
cookie_secure = false

[security]
require_mfa = false

[update]
enabled = false

[analytics]
engine = "embedded"
```

`embedded` needs no `url` or `password_file`: Forge generates the engine's
password on first start and keeps it in `$D/data/analytics/secret`
(mode 600).

Install the engine binary. `forge-server analytics install` checks the
SHA-256 you pass and copies the file to the default `embedded_binary`,
`<data_dir>/analytics/bin/clickhouse` (here
`$D/data/analytics/bin/clickhouse`). If the host already has the `clickhouse`
binary (for example `/usr/bin/clickhouse` from the official packages), use
that file instead of downloading.

The official build at `builds.clickhouse.com/master/amd64/` needs a CPU
with AVX2 and exits with `SIGILL` (status 132) on older ones. Check with
`grep -c avx2 /proc/cpuinfo`; when it prints `0`, use `amd64compat` in the
URL instead of `amd64`. The download compresses itself: make it executable
and run it once so it unpacks in place, then hash the unpacked file:

```sh
mkdir -p /tmp/forge-ch && curl -fL -o /tmp/forge-ch/clickhouse \
  https://builds.clickhouse.com/master/amd64/clickhouse     # or .../master/amd64compat/clickhouse
chmod +x /tmp/forge-ch/clickhouse
/tmp/forge-ch/clickhouse --version                          # unpacks; prints the version
SUM=$(sha256sum /tmp/forge-ch/clickhouse | cut -d' ' -f1)   # builds.clickhouse.com publishes no checksum
target/debug/forge-server --config $D/forge.toml analytics install \
  --binary /tmp/forge-ch/clickhouse --sha256 "$SUM"
```

The command prints `installed the analytics engine at …`. The engine starts
with the server in step 5.

## 5. Initialise and start the server

Create the admin account once, with a generated password kept in a file:

```sh
umask 077
PW=$(python3 -c "import secrets;print(secrets.token_urlsafe(18))")
echo "$PW" > $D/admin-password.txt
FORGE_ADMIN_PASSWORD="$PW" target/debug/forge-server --config $D/forge.toml init --username admin
```

Start it detached, so it outlives the shell that started it:

```sh
setsid nohup target/debug/forge-server --config $D/forge.toml serve >> $D/server.log 2>&1 < /dev/null &
```

Verify:

```sh
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:55130/api/health    # 200
grep -i analytics $D/server.log | tail -3     # no "not installed" or "unavailable" lines
curl -s -u forge:$(cat $D/data/analytics/secret) 'http://127.0.0.1:18123/?query=SELECT%20version()'
```

Sign in at `http://<host-ip>:55130` as `admin` with the password from
`$D/admin-password.txt`.

To restart after a code change: stop the process, `cargo build -p
forge-server`, start it again. Rebuild `src/ui` too if UI code changed.

## 6. Start the Docker nodes

Files: `scripts/dev/nodes/` (`Dockerfile`, `compose.yaml`, `entrypoint.sh`).
Each container downloads the agent from the server and enrolls with a
single-use token on first start; the identity is kept in a named volume.

1. Create one token per node. This also creates cluster `dev`:

   ```sh
   cd scripts/dev/nodes
   umask 077
   S=../../../target/debug/forge-server; C=$D/forge.toml
   { for n in 1 2 3; do
       echo "NODE${n}_TOKEN=$($S --config $C enroll-token --cluster dev --hostname dev-node-$n --ttl-minutes 60)"
     done; } > .env
   ```

   `.env` is git-ignored. The tokens are needed only for the first start.

2. Build and start:

   ```sh
   docker compose up -d --build
   docker compose ps
   ```

3. Verify: the three nodes show as online under **Nodes** in the console,
   or:

   ```sh
   docker compose logs --tail 5 node-1
   grep -i "dev-node" $D/server.log | tail -3
   ```

## 7. Deploy the portals (services, database, ingress)

From the repository root:

```sh
python3 -u scripts/dev/nodes/seed_portals.py
```

The script is idempotent and does this through the Forge API:

1. Installs PostgreSQL from the App Store on `dev-node-3`.
2. Creates the `portals` role and database (via `docker exec`). The password
   is generated once into `scripts/dev/nodes/.portals-db-password`
   (git-ignored).
3. Creates project `portals`, environment `dev`, in cluster `dev`.
4. Creates services `py-portal` (Python, port 8101) and `net-portal`
   (.NET 10, port 8103), active on `dev-node-1` and `dev-node-2`. Forge
   installs the runtimes.
5. Uploads the sources from `scripts/lab/portals/`, creates a log ingest key
   per service and deploys.
6. Creates an Nginx ingress on `dev-node-3`, round robin to nodes 1 and 2,
   published on the host.

Overrides (environment variables): `FORGE_API`, `FORGE_LOGIN`,
`FORGE_PASSWORD_FILE`, `FORGE_NODE_URL`, `FORGE_DEV_DOMAIN` (ingress
host suffix, default `127.0.0.1.nip.io`) and `FORGE_DEV_EDGE_PUBLISH`
(where the `forge-dev-edge-proxy` socat container publishes the edge,
default `127.0.0.1:8088`). Set both to `<host-ip>` when this host has no
`127.0.0.1` address. If the portals log `401` when
shipping logs, rerun with `FORGE_ROTATE_LOG_KEYS=1`.

Verify:

- **Projects** shows `portals` → `dev` with both services running on two
  nodes.
- **Logs** in the console shows lines from `py-portal` and `net-portal`
  (source `ingest`). PostgreSQL, Nginx and the portals run as processes
  inside the node containers, not as containers of their own, so the
  container lines (source `container`) come from the `forge-dev-nodes-*`
  containers. Every node reads the shared host engine, so each container
  line arrives once per node.
- The lines are in the embedded engine. It listens on loopback only, as
  user `forge` with the generated password:

  ```sh
  curl -s -u forge:$(cat $D/data/analytics/secret) http://127.0.0.1:18123/ --data-binary \
    "SELECT service_id, source, count() FROM forge_op_analytics.app_logs GROUP BY service_id, source FORMAT TSV"
  ```

  Match `service_id` to a name with `forge services list` or the service
  page's id. The CLI signs in through a browser; on a headless host create
  an API key under **Profile → Tokens & API keys** (or `POST /api/v1/api-keys`) and
  pass it in the environment:

  ```sh
  cargo build -p forge-cli
  FORGE_SERVER=http://127.0.0.1:55130 FORGE_TOKEN=<api-key> target/debug/forge services list
  ```

## 8. Load testing (optional)

PowerShell 7 (`pwsh`) scripts in `scripts/dev/nodes/`:

- `load-portals.ps1`: traffic through the ingress to both portals; prints
  throughput, latency percentiles and the split per node.
- `load-logs.ps1`: log ingest load.

Run `Get-Help ./load-portals.ps1 -Full` for parameters.

## 9. Tear down

```sh
pgrep -ax forge-server                               # find the pid of `target/debug/forge-server ... serve`
kill <pid>                                           # stop the server; the engine child stops with it
(cd scripts/dev/nodes && docker compose down -v)     # nodes and their identities
docker rm -f forge-dev-edge-proxy                    # the ingress port forward seed_portals.py starts
docker ps --filter label=com.docker.compose.project  # leftover app containers the nodes deployed on the host engine; remove as needed
rm -rf ~/.local/share/forge-dev                      # all server data, analytics included (destructive)
```

Do not stop the server with `pkill -f "forge-server --config"`: the pattern
also matches the shell running the command and kills it. To reuse the engine
binary after the wipe, copy `$D/data/analytics/bin/clickhouse` somewhere else
first and install it again in step 4.

To reset only the nodes, run `docker compose down -v`, create new tokens
(step 6.1) and start again. Remove the old node records from **Nodes**
first.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `permission denied ... docker.sock` | Login session lacks the `docker` group | Prefix with `sg docker -c "..."` or log in again |
| Node never enrolls; logs show download errors | Agent binaries missing from `$D/dist` | Redo step 3 |
| Node enrolls once, fails after `down -v` | Token is single-use | New token (step 6.1) |
| Console works locally but not remotely | `public_url` does not match the address you browse | Fix `public_url`, restart the server |
| `the embedded analytics engine is not installed; run forge-server analytics install` in `server.log` | No binary at `$D/data/analytics/bin/clickhouse` | Do the install in step 4, then restart the server |
| `embedded analytics engine exited ... unix_wait_status(132)` in `server.log` | The engine build needs AVX2, which the CPU lacks | Install the `amd64compat` build (step 4), then restart the server |
| `SHA-256 mismatch` from `analytics install` | The `--sha256` value is not the file's | Recompute it with `sha256sum` on the same file |
| `analytics write failed ... MEMORY_LIMIT_EXCEEDED` in `server.log` | The engine's memory budget (`embedded_memory_mb`, 1024 MiB by default) is reached | Rows stay buffered and retry; raise the budget under **Settings → Analytics** if it repeats |

See also `docs/src/content/docs/development/docker-nodes.md` and
`run-from-source.md`.
