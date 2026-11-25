# Docker Swarm end to end

Branch `feat/swarm-end-to-end`. Builds on D-154 (cluster modes), D-156
(strategies on Swarm) and D-157 (typed Docker operations). Decision record:
D-171.

## Where Swarm stood

Single container services work: replicas/global, placement, spread, ports,
health check, restart, limits, mounts, rollout, rollback, scale, restart,
tasks, logs, blue-green/canary through paired services. Membership (init,
join, leave, promote/demote, drain, labels, token rotation) works from the
console and the API. Everything else a Swarm user expects was missing.

## Gaps

| # | Gap | Done |
| --- | --- | --- |
| 1 | Compose stacks on Swarm (`docker stack deploy`), redeploy, remove, per-stack view | `swarm.stack` job, stack API, console, CLI |
| 2 | Swarm secrets and configs, versioned rotation, attached to services and stacks | For stacks (external refs resolved to the newest version); API, console, CLI. Single services: not yet (see "Not done") |
| 3 | Shared attachable encrypted overlay per cluster, network list/inspect | `forge-shared`, network list with subnets and attached services |
| 4 | Advertise/data-path address on the mesh address | suggested addresses (mesh first) in the API and the join form; `DataPathAddr` sent with the advertise address |
| 5 | Volumes: list/create/remove per node with nfs/cifs options, used-by | typed NFS/CIFS options on volume create (D-157 extended), used-by in the swarm volume list |
| 6 | CLI parity | `forge swarm …` (nodes, tokens, stacks, secrets, configs, networks, volumes, services) |
| 7 | Visibility: per-task CPU/memory, task history, health summary | stack/service health summary, task history (all task states). Per-task CPU/memory: not yet |
| 8 | Convert Default → Swarm | guided pre-check + convert endpoint; Swarm → Default refused while a swarm exists |
| 9 | Ingress endpoints to Swarm services and stack services | stack services with published ports offered as targets on every swarm node (routing mesh) |

## Design

### Stacks on Swarm (gap 1)

*Extend, don't fork.* A stack keeps its row, revisions and sealed `.env`
(`forge_core::stacks`). When its environment belongs to a Swarm cluster the
deploy goes to a manager as one `docker stack deploy` instead of one
`docker compose up` per node.

- `forge_catalog::swarm_stack::SwarmStackPart` is the manager's part:
  environment, stack, compose tree, `.env`, policy, secret/config versions.
  `prepare()` interpolates, runs the **same** `stacks::check` policy
  (privileged, binds, sockets, `build:` blocked), then the Swarm rules:
  - top-level `secrets:`/`configs:` must be `external: true`; Forge rewrites
    `name:` to the newest Forge-made version (`<name>.v<N>`). `file:`
    entries would read files on the manager and are refused.
  - services that publish `ports:` (or set `x-forge.shared: true`) also join
    the external `forge-shared` network; every service stays on the stack's
    own encrypted overlay (`default`, `driver: overlay`, `encrypted`).
    Services without ports (redis, seq…) are reachable only inside the stack.
  - `depends_on` is dropped with an info finding (Swarm ignores it; health
    checks and restart policies do the work).
  - `container_name`, `network_mode: host` and `pid/ipc: host` are refused
    (not supported by Swarm or a host escape).
  - every service gets `deploy.labels.forge.stack=<environment>/<stack>`.
- The helper writes `/opt/forge/swarm-stacks/<project>/compose.yaml` (0600,
  root) and runs `docker stack deploy --prune --with-registry-auth
  -c compose.yaml <project>` with a cleared environment, like compose stacks.
  The namespace is `project_name(environment, stack)`.
- `swarm.stack` job: `prepare` (manager, shared network), `apply`,
  `converge` (all services' running = desired; a paused update fails).
  Remove is `docker stack rm`, then waits until the services are gone.
  All steps are idempotent: `stack deploy` converges to the file.

### Secrets and configs (gap 2)

Docker is the record: Forge labels what it makes (`forge.managed`,
`forge.object.name`, `forge.object.version`). Values come from the request
(or a vault item) and go only into the signed command to the manager,
base64-encoded; they are never stored, logged, audited or returned.
Secrets are immutable in Swarm, so *rotate* creates version N+1; the next
stack deploy picks it up (a rolling update), and **prune** removes versions
no service uses. Managing them is `swarm.manage` (admin, MFA).

### Networks (gap 3)

`forge-shared`: attachable, encrypted overlay made by Forge in every swarm
(`EnsureSharedNetwork`, run by every stack deploy and on demand). Attaching
is governed by `stack.manage`/`deploy` on the environment that deploys. The
network list reads `GET /networks` + services on a manager: scope, driver,
subnets, encrypted, attachable, ingress flag and attached services.

### Mesh addresses (gap 4)

`GET /clusters/{id}/swarm/addresses?server_id=` returns the node's known
addresses with the fabric (WireGuard) address first when the cluster's
workspace has a fabric that includes the node. The join form preselects it.
The agent sends `DataPathAddr` equal to the advertise address when it is an
IP, so VXLAN also runs over the mesh. Without a fabric: LAN behaviour.

### Convert Default → Swarm (gap 8)

`GET /clusters/{id}/convert/check` and `POST /clusters/{id}/convert
{mode:"swarm"}`. Pre-checks: every node Linux, no systemd services and no
node-pinned container services in the cluster's environments (they would
lose their scheduler), no stacks deployed per node. Conversion flips the
mode only; the operator then creates the swarm from the Swarm tab. Swarm →
Default is refused while a swarm row exists (leave all nodes first), then
allowed with the same checks reversed (no swarm services).

## Tests

- Unit: catalog validation and stack rewriting, agent views (networks,
  secrets, stack services), progress logic.
- Integration (`tests/it/swarm.rs`, real SQLite): stack deploy goes to a
  `swarm.stack` job on Swarm environments, secrets API authz + no value in
  audit, convert pre-checks, addresses ordering.
- End to end: 3 `docker:dind` nodes with the agent, a local server.

## Status

All nine gaps are implemented on this branch (backend, agent, API, CLI,
console, docs, tests). Secrets and configs also work on single Swarm
services (`swarm.secrets`, `swarm.configs`, `swarm.secret_env: files`), and
per-task CPU/memory comes from one `TaskStats` command per node that runs the
viewed tasks (cached 10 s).

## End-to-end run (3 Docker-in-Docker nodes, CLI only)

Nodes `lab-a` (manager), `lab-b`, `lab-c` as privileged Debian containers
running dockerd 29 and the agent built from this branch, enrolled with
`forge nodes enroll-token --cluster swarm-e2e`. Every step below used the
`forge` CLI (one bootstrap API key was minted with the password once):

- `swarm addresses`, `swarm init` (advertise address), `swarm join` ×2,
  `swarm nodes`, `swarm labels` → 3-node swarm, leader on lab-a.
- `swarm secrets create`, `swarm configs create`, `swarm networks shared`.
- `stacks create` (shop: api+worker with ports, redis+seq without) →
  policy notes for `depends_on`/`container_name`; `swarm stacks deploy` →
  `swarm.stack` job succeeded; `swarm stacks ps` all replicas running.
- Probe stack in another environment (`x-forge.shared`): reached
  `forge-prod-shop_api` and `_worker` by name across nodes; `_redis` did not
  resolve and `_seq` was unreachable; inside the stack `seq` answered by name.
- Secret file present in the api task with the value; `docker service
  inspect` shows only the secret reference.
- `swarm stacks scale api 3`; `swarm secrets rotate` + `stacks update` +
  `swarm stacks diff` + deploy → rolling update, env `RELEASE=2`, secret
  `db_password.v2`; `swarm stacks rollback api` → `rollback_completed`, back
  to v1; redeploy revision 2; `swarm stacks restart worker`; `swarm stats`;
  `swarm stacks logs`.
- `swarm drain lab-a` → 0 tasks left on lab-a, all rescheduled on lab-b/c;
  `swarm activate`; `swarm networks ls` (ingress, stack overlays encrypted,
  `forge-shared` attachable); `swarm secrets prune` removed `db_password.v1`.
- `sites create` with a `swarm` target → upstreams on all three nodes
  `:18080`; the routing mesh answered on every node.
- `docker volume create` (local and NFS), `swarm volumes` with used-by;
  `swarm convert --check` lists the blocking reason.

DinD note: nested overlayfs fails (`failed to mount ... overlay`); the nodes
ran dockerd with the `vfs` driver.

## Not done / owner decisions

- The ingress edge (Nginx) was not applied in the DinD run: the nodes carry
  no Nginx. The endpoint's resolution to every swarm node and the routing
  mesh itself were verified.
- The console was built and type-checked but not driven in a browser (no
  headless browser binary on this machine).
- Swarm → Default with running Swarm services stays a guided refusal: the
  check names what to remove and the guide lists the manual steps.
- Task memory reads 0 MiB in DinD (nested cgroups); real nodes report it.
