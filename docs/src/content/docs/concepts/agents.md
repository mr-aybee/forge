---
title: Agents
description: Enrollment, the mTLS link, signed commands, local policy, rotation and revocation, the privileged helper, and how agent rollouts and automatic updates work.
sidebar:
  order: 5
---

## Enrollment

```diagram
alt: Agent enrollment sequence
sequence:
  actors: [Operator, Agent, Control plane]
  messages:
    - { from: Operator, to: Control plane, label: "create token" }
    - { at: Control plane, note: "token = `<secret>.<sha256(CA)>`" }
    - { from: Agent, to: Control plane, label: "GET /agent/v1/ca" }
    - { at: Control plane, note: "TLS, chain not yet trusted" }
    - { from: Control plane, to: Agent, label: "CA PEM" }
    - { at: Agent, note: "agent checks sha256 == pin" }
    - { from: Agent, to: Control plane, label: "POST /agent/v1/enroll {secret, CSR}" }
    - { at: Control plane, note: "verified TLS" }
    - { from: Control plane, to: Agent, label: "{server_id, cert, command public key}" }
    - { at: Control plane, note: "token consumed atomically" }
```

- The private key is generated on the server and never leaves it.
- The certificate's identity is the **server id assigned by the control plane**;
  the CSR's subject is ignored.
- Tokens are single use, expire (1–1440 minutes) and are stored only as hashes.

## The link

One HTTP/2 connection over mutual TLS, opened by the agent:

| Endpoint | Purpose |
| --- | --- |
| `POST /agent/v1/heartbeat` | Long-poll. Carries results and facts; returns signed commands as soon as one is queued, or after the hold time |
| `POST /agent/v1/renew` | New CSR → new certificate |

Messages are protobuf (`src/forge/crates/protocol/forge-proto/proto/forge_agent_v1.proto`).
On failure the agent reconnects with exponential backoff and full jitter
(1 s → 60 s), so a restarted control plane is not stampeded.

## Commands

Each command is an encoded `CommandBody` plus an Ed25519 signature. The agent
accepts it only if all of these hold:

1. the signature verifies with the command key received at enrollment;
2. `server_id` is this agent;
3. it is within its validity window (TTL ≤ 5 minutes, ±30 s skew);
4. it was issued after the agent started (a restart cannot be used for replay);
5. its operation id is not in the replay cache;
6. its kind is in the local allowlist in `agent.toml`.

```toml
# agent.toml — the server's owner has the final say
server_url = "https://forge.example.com:3843"
allowed_commands = ["ping", "collect_facts", "service_status"]
```

## Rotation and revocation

Certificates last 24 hours. When ⅔ of the lifetime has passed, the heartbeat
response asks the agent to renew with a fresh key. Forge renews only the
certificate it last issued to that node, and only once renewal is due; any
other certificate of the node gets `403`. After issuing the new certificate,
Forge adds the old serial to the revocation list, so the old certificate stops
working at once. Each renewal and each refused attempt is audited as
`server.cert.renew`. Revoking a server
(`POST /api/v1/servers/{id}/revoke`) adds its certificate serial to the
revocation list; its next request is refused. Removing a server
(`DELETE /api/v1/servers/{id}`) revokes it the same way and then deletes the
node and its data. The serial stays on the revocation list, so the old
certificate is refused even though the node is gone. Enrolling the machine
again creates a new node with a new id.

## Privileged helper

The agent runs as the unprivileged `forge-agent` user. Work that needs root
(packages, apps, sites, files, stacks, networks, updates) goes to
`forge-agent-helper`, a root service on a Unix socket that only root and the
agent user can open.

The agent does not tell the helper what to do. It forwards each command
exactly as Forge signed it, and the helper:

1. checks the signature with Forge's command key and the target with this
   node's id, both pinned in its root-only folder
   `/var/lib/forge-agent-helper/pin.json`;
2. checks the command's lifetime and clock skew and refuses a replay, also
   after the helper restarts;
3. works out the request from the command itself, validates every argument
   and runs it. There is no generic "run this" request and no shell.

Once a pin exists, the helper refuses any request that is not signed by
Forge. The one exception is the agent confirming that it came back after a
self-update.

| How the pin is made | When |
| --- | --- |
| The installer runs `forge-agent-helper pin` as root right after enrolling | Every node enrolled with this release or later |
| The helper copies the key from the agent's `identity.json` the first time it starts | Nodes enrolled earlier, after their first agent update |

:::caution
A pin made on first use trusts a file the agent user owns. If you suspect a
node was compromised before its first update to this release, revoke it on
the node page and [add it again](/docs/guides/add-nodes/) with a new token: the
installer pins the new enrollment as root.
:::

On Windows the agent runs as LocalSystem and does privileged work in its own
process, so there is no separate helper boundary there.

## Rollouts and automatic updates

An agent update replaces the agent and helper on one node and rolls back on
its own if the new agent does not come back (see
[Update agents](/docs/guides/update-agents/)). A rollout applies that same update
to many nodes, a few at a time, so that a bad release stops on the first
nodes instead of reaching the whole fleet.

```diagram
alt: "agent.rollout: batches of agent updates"
caption: "`agent.rollout` (one step: roll). Nodes that cannot take the update are skipped; the first real failure stops it."
flow:
  rows:
    - title: batch 1 (canary)
      steps:
        - { label: "batch 1", edge: "" }
        - { label: "`agent.update` × N", edge: "" }
        - { label: "every node on the target?", tone: accent }
    - title: batch 2
      steps:
        - { label: "batch 2", edge: "" }
        - { label: "`agent.update` × N", edge: "" }
        - { label: "every node on the target?", tone: accent }
    - title: batch n
      steps:
        - { label: "batch n", edge: "" }
        - { label: "`agent.update` × N", edge: "" }
        - { label: done, tone: ok }
```

- **One update path.** The rollout is a durable parent job. Each node still
  gets an ordinary `agent.update` job, with the same signed command, checks
  and automatic rollback as a single update. The rollout only decides when
  to start the next batch.
- **Canary first.** The first batch is the canary. The next batch starts
  only when every node of the current one is back on the target version or
  was skipped. A failed node, a node Forge could not queue, or a batch that
  runs past its timeout stops the rollout. Nodes in later batches keep their
  working agent.
- **Skip, don't stop.** A node that cannot take the update says why with a
  refusal code, and an offline node gets until the end of its batch
  (`[update] agent_batch_timeout_secs`). The rollout skips such nodes and
  lists them with the reason. Only a real failure, such as a new agent that
  does not come back, is evidence against the release, so only that stops
  the rollout. Cancelling a rollout stops its waiting updates too.
- **Checked up front.** Forge checks `server.command` for every node before
  it queues the rollout. A rollout never stops halfway because of a
  permission it lacked, and each update job checks it again when it runs.
- **Safe to resume.** The `roll` step can run again after a crash. Each
  update job it starts is keyed by node, version and rollout, so a re-run
  finds the jobs it already started instead of starting them twice. A new
  rollout gets new keys, so it can retry a node that failed in an earlier
  one.

Because the rollout waits on its own update jobs, the control plane must
run at least two jobs at once (`jobs.concurrency` of `2` or more).

### Automatic updates

Automatic updates are a rollout that Forge starts for you. They are off by
default. When they are on, Forge checks every 5 minutes. It starts a rollout
when the time is inside the weekly maintenance window, nodes are behind the
control plane's version and no rollout is running.

The window is in UTC. A window whose end is past midnight belongs to the day
it starts, so Saturday `23:00`–`01:00` covers the first hour of Sunday.

Two design choices keep this safe:

- **It acts as a person.** The rollout runs as the person who turned
  automatic updates on, with that person's permissions and MFA state, and
  Forge authorizes it like any job they start. There is no service account
  with standing rights over every node. If that person loses
  `server.command` on a node, the next automatic rollout fails to start
  instead of running with rights nobody granted. The setting covers every
  node, so changing it needs `system.update` on the platform (platform
  admins) and an MFA-verified session, because whoever saves it becomes the
  person the rollouts run as. That MFA counts for 90 days; after that,
  automatic rollouts stop until someone saves the setting again. The generic
  settings editor refuses keys that start with `agents.`, so that check
  cannot be bypassed.
- **One try per version.** Forge starts at most one automatic rollout for
  each new version. If it fails, Forge does not try that version again in
  the next window. A canary that failed is a signal to look; repeating the
  rollout every window would push a bad release onto more nodes or keep
  restarting the same broken one. A person reads the failure and starts the
  next rollout by hand.
- **One rollout at a time.** A rollout is refused (`409`) while another is
  queued or running, whether it was started by hand or automatically, and a
  node with an update already queued or running is refused too. A waiting
  rollout holds a job worker, so several at once could stall other jobs.

Automatic starts and failures are audited as `agent.auto_update.start` and
`agent.auto_update.failed`, with the actor `system:forge`. The rollout job
itself carries the enabling person as its principal.

:::note
Self-update is for Linux nodes with systemd. Windows nodes are left out of
rollouts and automatic updates; update them by running the installer again.
:::
