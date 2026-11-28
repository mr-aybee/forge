---
title: Linux test lab (WSL)
description: A control plane plus three 1 CPU / 1 GB Linux nodes in WSL for onboarding tests.
sidebar:
  order: 5
---

:::note
Not in use for now: tests run on the [Hyper-V VM lab](/docs/development/hyperv-lab/),
with no containers on the developer machine. This page is kept for when the
container lab is needed again.
:::

`scripts/lab/` runs a self-contained lab on Windows using WSL and Docker
Engine (no Docker Desktop):

| Container | Role | Limits |
| --- | --- | --- |
| `forge-cp` | Control plane (static musl build), UI on `localhost:8090` | 1 CPU, 1 GB |
| `node-1`…`node-3` | "Fresh VMs": Debian slim, SSH, user `forgeops` with sudo (password required), root login disabled, no systemd | 1 CPU, 1 GB each |

## Setup (once)

```powershell
wsl -d Ubuntu-24.04 -u root -- bash scripts/lab/setup-wsl.sh <your-wsl-user>
```

Installs `docker.io`, build tools, `musl-tools` and Rust with the
`x86_64-unknown-linux-musl` target.

## Build and start

```powershell
wsl -d Ubuntu-24.04 -- bash scripts/lab/build.sh   # static binaries → scripts/lab/.lab/
(cd src/console; npm run build)                         # the console
wsl -d Ubuntu-24.04 -- bash scripts/lab/up.sh
```

Seed it with clusters, nodes, environments, services, network plans and a vault
credential (safe to re-run):

```powershell
wsl -d Ubuntu-24.04 -- python3 scripts/lab/seed.py
```

This creates:
- the clusters `lab` (node-1, node-2) and `uat` (node-3), all three enrolled with the real installer;
- the environments `payments` (protected) and `shop` in cluster `lab`, and `payments-uat` in cluster `uat`;
- the services `api`, `worker` (scheduled) and `web` (container);
- a suggested /16 network for each cluster;
- a vault credential linked to node-1.

`up.sh` prints the admin login and the node login (generated once, stored in
`scripts/lab/.lab/credentials.env`, git-ignored). Open
`http://localhost:8090` in a browser.

## Test plan

1. Sign in, enable MFA (Profile), sign in again with a code.
2. Create a cluster.
3. **node-1, assisted:** Add node → Assisted → host `node-1`, port 22 →
   confirm fingerprint (`docker exec node-1 ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub`)
   → user `forgeops` + password → pick the cluster → join.
4. **node-2, manual:** Add node → Manual → generate token → run the command in
   the node: `docker exec -it node-2 bash`, then `su - forgeops` and paste.
5. **node-3, failures:** wrong password (job fails: authentication), changed
   host key (recreate the container, reuse the old fingerprint → refused).
6. Ping and collect facts on each node; check the vault is empty and the
   audit chain verifies.

## Stop and reset

```powershell
wsl -d Ubuntu-24.04 -- bash scripts/lab/down.sh          # stop
wsl -d Ubuntu-24.04 -- bash scripts/lab/down.sh --reset  # delete containers and data
```

## Ingress scenarios (mock portals)

`scripts/lab/scenarios.py` deploys three mock portals and checks the ingress
end to end. Each portal is one page saying which node and IP served it (add
`?format=json` for the machine-readable form):

| Service | Runtime | Port | Domain |
| --- | --- | --- | --- |
| `py-portal` | Python 3 | 8101 | `py.lab.local` |
| `php-portal` | PHP 8.3 (`php -S`) | 8102 | `php.lab.local` |
| `net-portal` | .NET 10 SDK, file-based app | 8103 | `net.lab.local` |

They live in environment `portals` of cluster `lab`, active on node-1 and node-2.
Forge installs the runtimes during deploy. The sources come from
`scripts/lab/portals/` and are copied into the containers. This is a lab
shortcut; on real nodes you would deploy them.

```bash
wsl -d Ubuntu-24.04 -- python3 scripts/lab/scenarios.py all
# or one step: portals | standalone | ha | failover | verify
```

1. **standalone:** Nginx on node-1, endpoints for the three domains. Twelve
   requests per domain through node-1 must reach both node-1 and node-2.
2. **ha:** Nginx on node-1 and node-2 (active-active), PowerDNS on node-3 with
   zone `lab.local`. Forge publishes A records listing both edges, and both
   edges balance across both nodes.
3. **failover:** `docker pause node-2` (link down). Within about a minute the
   records list only node-1. After `docker unpause` node-2 comes back, and
   both moves appear in the audit log as `ingress.dns.failover`.

To browse a portal from Windows, point the domain at the edge in your hosts
file, or use PowerDNS on node-3 as your resolver.

### One command: the testbed

`scripts/lab/testbed.sh` rebuilds the whole setup and makes it reachable from
the Windows browser. Every step reuses what already exists, so it is safe to
re-run.

```bash
wsl -d Ubuntu-24.04 -- bash scripts/lab/testbed.sh            # lab, portals, one Nginx, browser forward
wsl -d Ubuntu-24.04 -- bash scripts/lab/testbed.sh ha         # two Nginx + PowerDNS + DNS failover
wsl -d Ubuntu-24.04 -- bash scripts/lab/testbed.sh failover   # link-down test
wsl -d Ubuntu-24.04 -- bash scripts/lab/testbed.sh proxy      # only the browser forward
wsl -d Ubuntu-24.04 -- bash scripts/lab/testbed.sh fresh      # rebuild, reset the lab, then the default steps
```

The browser forward is a small `alpine/socat` container (`lab-edge-proxy`).
It sends Windows `localhost:8088` to node-1's Nginx. Add this line to
`C:\Windows\System32\drivers\etc\hosts` (as administrator):

```
127.0.0.1  py.lab.local php.lab.local net.lab.local
```

Then open `http://net.lab.local:8088` (also `py.` and `php.`). Refresh to see
the serving node alternate. `TESTBED_PORT` and `TESTBED_EDGE` change the port
and the edge.
