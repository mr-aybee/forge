---
title: Deployments and releases
description: How Forge turns a forge.deploy.yaml into an immutable release and rolls it out to a service's nodes with slots, traffic weights, health gates and rollback.
sidebar:
  order: 16
---

This page explains how Forge rolls a new version of a service out to its
nodes: what a release is, how two copies run side by side, how traffic moves
between them, and what happens when something fails. For the steps, see
[Deploy a service from YAML](/docs/guides/deploy-with-yaml/); for every field,
see [Deployment YAML](/docs/reference/deployment-yaml/).

## Releases and deployments

A **release** is an immutable, numbered snapshot of one service: its runtime
fields, its environment variables (secret values sealed as they were when
the release was made), its artifact and its rollout policy. Releases are
numbered per service from 1. The YAML text the release was made from is kept
with it, with its SHA-256.

A **deployment** is one rollout of one release. It has a kind (`apply`,
`rollback` or `auto_rollback`), a plan, a state and an event log. Applying a
document creates a release and a deployment; redeploying or rolling back
creates a deployment of an existing release.

```diagram
alt: Deployment pipeline and states
caption: "`forge.deploy.yaml` → release #7 (snapshot) → deployment → plan → steps on nodes"
states:
  path:
    - { state: pending, edge: "" }
    - { state: running, dir: both, tone: accent }
    - { state: paused }
  transitions:
    - { from: running, to: awaiting_approval, when: "⇅ approval gate" }
    - { from: running, to: succeeded, tone: ok }
    - { from: running, to: failed, tone: danger }
    - { from: running, to: aborted, tone: muted }
    - { from: running, to: rolled_back, tone: warn }
```

When a deployment succeeds, the service becomes the release: its stored
spec and environment are replaced by the release's. The release that served
before is recorded on the deployment as `from_release`.

A YAML deployment wraps a service: it does not introduce a new kind of
object. The service still appears on its environment's page, and the
**Deploy** action on the service page still deploys the service's current
definition in place.

## Slots

To start a new release without stopping the old one, a node runs two copies
of the service side by side, in **slots** `a` and `b`:

| Copy | Container or systemd unit | Host ports |
| --- | --- | --- |
| Unslotted instance | `forge-app-<env>-<service>` | The service's own ports |
| Slot `a` | `forge-app-<env>-<service>--a` | Its own ports from 20000–29999 |
| Slot `b` | `forge-app-<env>-<service>--b` | Its own ports from 20000–29999 |

Each slot gets one host port per published container port (or one for a
native service's `listenPort`) from the pool 20000–29999. A slot keeps its
ports across releases, and no two services share one. A native service in a
slot gets its port in `$PORT` and must listen on it.

A new release goes into the slot that does not serve now (`a` if nothing
serves yet). A service deployed before slots existed runs as the unslotted
instance; its first slotted rollout starts slot `a` next to it and stops it
once traffic has moved.

Strategies that never run two copies at once (`recreate`, and `rolling` or
`manual` with `replacement: stop-first`) deploy in place: into the slot that
serves now, or into the unslotted instance.

:::note
Slot ports are reached only through a Forge endpoint. Edge nodes must be
able to reach the service's nodes on 20000–29999. Clients that connect to
a node's own port directly do not follow a slotted rollout, which is why
the planner refuses slotted strategies for a service no endpoint routes to.
:::

## Plans and safety refusals

Before anything starts, the planner turns the policy and the current state
of each node into an ordered list of steps:

| Step | Does |
| --- | --- |
| `start` | Deploys the release into the listed copies (in parallel) |
| `gate` | Waits until the copies are ready, then stable |
| `route` | Sets traffic weights and re-applies the service's endpoints |
| `wait` | Holds for draining or a canary pause, with optional traffic gates |
| `approve` | Waits for an approval |
| `stop` | Stops and removes old copies |

`forge deploy plan` and the console's **Plan** button show this list
without starting anything. The planner refuses, rather than risk an outage,
when:

- a node of the placement is offline;
- the strategy runs two copies side by side but no endpoint routes to the
  service (the new copy's ports would be unreachable);
- a `job` would run in slots (a job takes no traffic);
- stop-first would take every running copy down at once and
  `allowDowntime` is not set.

`recreate` is allowed but the plan says `downtime: yes`.

## Strategies

| Strategy | Copies at once | Traffic moves | Use it for |
| --- | --- | --- | --- |
| `rolling`, `start-first` (default) | Old and new on the nodes of one batch | Batch by batch, 0 → 100 % per node | Most services behind an endpoint |
| `rolling`, `stop-first` | One per node | Node leaves traffic, is replaced, comes back | Services that cannot run twice on a node, or have no endpoint; needs two or more nodes or `allowDowntime` |
| `recreate` | One per node | All nodes replaced at once | Jobs, singletons, schema changes that cannot run mixed; causes downtime |
| `blue-green` | Old and new on every node | All at once, after every node is ready | Fast, clean switch with an instant way back until the old copies stop |
| `canary` | Old and new on every node | In weighted steps (`10`, `50`, `100` %), with pauses, approvals and traffic gates | Risky changes you want to measure on a share of traffic |
| `manual` | As rolling | Batch by batch, each batch after an approval | Changes someone watches |

Batches are `maxSurge` nodes (start-first) or `maxUnavailable` nodes
(stop-first), or `batchSize` when set. `approval.required` adds one approval
before traffic first moves to the new release.

## Traffic switching

Traffic moves through Forge **endpoints** (sites served by Nginx on edge
nodes). Each copy has a weight from 0 to 100. A `route` step writes the
weights, then re-applies every endpoint that routes to the service:

- copies with weight 0 are left out of the upstream list (drained);
- other copies are listed with their slot ports and an Nginx `weight` when
  it is not 100;
- each edge tests the configuration before it reloads.

The re-apply runs as whoever applied that endpoint last, as certificate
renewals do, so a deployer needs no rights on the endpoint itself. An
endpoint that was never applied is left alone.

After traffic leaves a copy, the plan waits `drainTimeout` so requests in
flight finish, then stops the copy.

For canary gates, the edges' traffic log records the upstream status and
response time of each request (`us=$upstream_status
rt=$upstream_response_time`). Forge counts 5xx answers and failed
connections as errors and computes the 95th percentile per upstream over
the last minute.

## Health gates

A `gate` step asks each new copy's node to probe it now, without waiting
for the regular health cycle. The probe runs the default check (unit active
or container running, plus a TCP connect to the listen port) and each
required HTTP and TCP readiness check of the workload, moved to the slot's
ports. A copy passes when it is ready and then stays ready for
`stabilizationPeriod`. If it is not ready and stable within
`startupTimeout`, the step fails.

The node's regular health runner follows the copy that serves: when traffic
moves to another slot, Forge sends the node the service's policy for that
slot. The policy's revision stays the service's own, so the node never
reports a revision Forge does not know.

Agents too old to answer a probe fail the gate with a message to update the
agent.

## Durability, locking and crash resume

The rollout runs as one durable job, `deploy.rollout`. The plan is stored on
the deployment, and `step` counts the steps done. Each finished step is
saved before the next starts. After a server restart or a lost job lease,
the job continues with the step it was on. Every step is safe to repeat: a
start deploys the copy again, a gate probes again, a route writes the same
weights, a stop of a copy already gone does nothing.

Only one deployment per service runs at a time. A second apply, redeploy or
rollback answers `409 Conflict` until the first ends.

Pause, resume and abort are requests the job reads between steps and while
it waits. A pause during a wait takes effect after the wait. Abort stops a
wait at once.

## Failure and automatic rollback

When a step fails, or someone aborts or rejects the deployment:

1. New copies that never took traffic are stopped and removed.
2. The deployment ends `failed` or `aborted`.
3. Forge starts an automatic rollback to `from_release` when all of these
   hold: the deployment failed (not aborted or rejected),
   `rollback.automatic` is true, it was an `apply` (not itself a rollback),
   a release served before, and at least one copy of the new release took
   traffic. The failed deployment then ends `rolled_back`, and a new
   `auto_rollback` deployment appears in the history.

A rollback is an ordinary deployment of the older release, with that
release's own rollout policy, on the service's current nodes.

:::caution
A rollback restores the service's definition, artifact and environment as
the older release recorded them, including the sealed secret values of that
time. It does **not** undo database migrations, data written by the newer
release, files outside the release folder, or endpoint and DNS changes.
Make schema changes backward compatible, or use `recreate` with a manual
plan for the way back.
:::

## Approvals

Approval steps wait for a person with `deploy.approve` on the service
(roles `platform_admin`, `workspace_admin` and `operator`; not
`developer`). The person who started the deployment cannot approve or
reject it, so every approval involves two people. A deployment nobody
approves within `approval.timeout` fails.

## Permissions

| What | Cedar action | Scope |
| --- | --- | --- |
| Validate a document | signed in | — |
| Plan, apply, redeploy, roll back, pause, resume, abort | `deploy` | The service; for a new service, its environment |
| Add or remove nodes through `placement` | `server.manage` | Each node added or removed |
| Container volumes outside the app folder | `stack.privileged` (MFA) | The environment |
| Approve or reject | `deploy.approve` | The service; never on your own deployment |
| Read releases, deployments, slots | `view` | The service |

## Limitations

- Linux nodes only. Systemd services (`systemd`) need systemd;
  containers need Docker Engine.
- Traffic switching and canary analysis need a Forge endpoint on Nginx edge
  nodes. Without one, use `recreate` or stop-first.
- Canary analysis sees only traffic that passes through Forge endpoints.

## Related

- [Deployment YAML](/docs/reference/deployment-yaml/)
- [Deploy a service from YAML](/docs/guides/deploy-with-yaml/)
- [Deploy from CI](/docs/guides/deploy-from-ci/)
- [Health checks](/docs/concepts/health-checks/)
- [Jobs](/docs/concepts/jobs/)
