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

| # | Gap | Plan |
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

## Not done / owner decisions

- Per-task CPU/memory (gap 7): needs `docker stats` on every node that runs
  a task (stats are per node, not per swarm): a fan-out per view. Left for a
  follow-up so the console does not issue N helper calls per poll.
- Secrets on single (non-stack) services: the service editor would need a
  secrets section; stacks cover the case today.
- Swarm → Default with running swarm services is not automatic: Forge cannot
  move Swarm tasks back onto pinned nodes without a placement decision.
