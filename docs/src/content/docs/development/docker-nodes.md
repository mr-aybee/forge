---
title: Disposable Docker nodes
description: Run three throwaway Forge nodes as Docker containers on your development machine, enrolled into a local forge-server.
sidebar:
  order: 5
---

`scripts/dev/nodes/` starts three Debian containers that enroll as Forge
nodes in a local control plane. Use them to try projects, environments,
containers and compose stacks without VMs. Delete them with one command.

:::caution
Each node mounts the host's Docker socket. Containers a node deploys run on
the host's Docker Engine, and anyone who can control a node controls the
host's Docker, which is root on the host. Use these nodes on a development
machine only.
:::

## What works

| Feature | Works | Why |
| --- | --- | --- |
| Containers, compose stacks, App Store items that run as containers | Yes | Through the host's Docker Engine |
| Systemd services (`systemd` runtime) | No | No systemd in the container |
| Agent self-update | No | Needs systemd; recreate the node instead |
| Bind mounts in a compose stack (`./data`) | Partly | Docker resolves the path on the host, not in the node container |

All three nodes share one Docker Engine, so they see each other's
containers.

## Before you begin

- A local forge-server built from source, see
  [Run from source](/docs/development/run-from-source/).
- Docker Engine with Compose v2, and your user in the `docker` group.
- The `x86_64-unknown-linux-musl` Rust target.

## Configure the server

The nodes reach the host on the default Docker bridge gateway, `172.17.0.1`.
Add these keys to the server's `forge.toml` and restart it:

```toml
api_listen = "0.0.0.0:55130"
agent_listen = "0.0.0.0:55131"
node_public_url = "http://172.17.0.1:55130"
agent_public_url = "https://172.17.0.1:55131"
agent_hostnames = ["172.17.0.1", "localhost", "127.0.0.1"]
agent_dist_dir = "<data-dir>/dist"
```

`api_listen` on `0.0.0.0` also opens the API to your network.

## Build the agent

The server hands the agent to the nodes from `agent_dist_dir`:

```sh
CC_x86_64_unknown_linux_musl=gcc \
  cargo build --release --target x86_64-unknown-linux-musl -p forge-agent
cp target/x86_64-unknown-linux-musl/release/forge-agent <data-dir>/dist/forge-agent-linux-x86_64
cp target/x86_64-unknown-linux-musl/release/forge-agent-helper <data-dir>/dist/forge-agent-helper-linux-x86_64
```

`CC_x86_64_unknown_linux_musl=gcc` builds `ring`'s C code with the system
compiler when `musl-tools` is not installed.

## Start the nodes

1. Create one enrollment token per node. `enroll-token` creates the cluster
   if it is missing:

   ```sh
   cd scripts/dev/nodes
   umask 077
   S=../../../target/debug/forge-server
   { echo "FORGE_URL=http://172.17.0.1:55130"
     for n in 1 2 3; do
       echo "NODE${n}_TOKEN=$($S --config <data-dir>/forge.toml enroll-token --cluster dev --hostname dev-node-$n --ttl-minutes 60)"
     done; } > .env
   ```

2. Build and start the containers:

   ```sh
   docker compose up -d --build
   ```

On first start each container runs the server's `install.sh`, which uses its
no-systemd path. The node's identity is kept in a named volume. Later starts
download the agent again and run it without a token.

## Verify

- `docker compose logs node-1` shows
  `forge-agent installed and running (no systemd; respawn loop)`.
- **Clusters → dev → Nodes** lists `dev-node-1` to `dev-node-3` as
  **online**.

## Remove the nodes

```sh
docker compose down -v
```

`-v` deletes the node identities. The nodes stay in the console as offline
until you remove them there. Containers the nodes deployed keep running on
the host's Docker; remove them with `docker ps` and `docker rm -f`.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `not enrolled yet: set FORGE_ENROLL_TOKEN` | No token in `.env`, or the volume was deleted | Create a new token, put it in `.env`, run `docker compose up -d` |
| Installer fails with an invalid or expired token | Tokens are single use and expire | Create a new token |
| Agent download fails | `agent_dist_dir` is empty or the server is not reachable on `172.17.0.1` | Build the agent, check `api_listen` |
| Node stays offline | Agent certificate does not name `172.17.0.1` | Add it to `agent_hostnames` and restart the server |

## Next steps

- [Services](/docs/guides/services/)
- [Compose stacks](/docs/guides/compose-stacks/)
- [Add nodes](/docs/guides/add-nodes/)
