---
title: Deploy a stack from YAML
description: Describe a compose stack in forge.stack.yaml and roll it out with forge stacks plan and apply.
sidebar:
  order: 7
---

This guide shows you how to keep a compose stack's placement and rollout in
a `forge.stack.yaml` next to its compose file, and deploy it with
`forge stacks apply`. For every field, see
[Stack YAML](/docs/reference/stack-yaml/).

## Before you begin

- **Permissions.** `stack.manage` on the environment and `server.manage` on
  each node the stack runs on (the `operator` role has both; `developer` has
  only `stack.manage`). Stacks with host-access settings also need
  `stack.privileged` with an MFA-verified session.
- **Linux nodes with Docker**, enrolled and online. Compose stacks run on
  Linux nodes only.
- **An environment that is not on a Swarm cluster.** Swarm stacks use
  [`forge swarm stacks`](/docs/guides/docker-swarm/).
- **The CLI**, signed in (`forge auth login`).
- Optional: a [cluster network](/docs/guides/cluster-network/), so services
  reach each other by name across nodes.

## Steps

1. Next to your `compose.yaml`, write `forge.stack.yaml`:

   ```yaml
   apiVersion: forge/v1
   kind: Stack
   metadata:
     name: orders
     environment: prod
   spec:
     compose:
       file: compose.yaml
       envFile: prod.env
     placement:
       cluster: c1
     rollout:
       order: node-by-node
       healthTimeout: 5m
   ```

2. Check it offline:

   ```sh
   forge stacks validate -f forge.stack.yaml
   ```

   An error names the field, for example
   `spec.placement: set either nodes or cluster`.

3. See what an apply would do:

   ```sh
   forge stacks plan -f forge.stack.yaml
   ```

   The plan lists the nodes in rollout order, the services on each, whether
   each node is on the cluster network, and every finding of the compose
   policy.

4. Apply it and wait for the rollout:

   ```sh
   forge stacks apply -f forge.stack.yaml --wait
   ```

   Type `yes` when asked, or pass `--yes`. Forge saves a new revision when
   the compose text changed and deploys the nodes one at a time.

## Verify

- The command ends with `stack orders revision <n> is live on <k> nodes`
  and lists the stack's names when the cluster network is in sync.
- `forge stacks show orders --environment prod` shows every node
  `deployed` at that revision.
- `forge stacks revisions orders` shows the revision with the note
  `apply <hash>`.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| Exit `1`, `placement.nodes lists several nodes; …` | Several nodes and no service names its node | Add `x-forge.node` to the services, or use `placement.cluster` |
| Exit `1`, `metadata.environment: no environment …` | The slug does not match an environment | Use the environment's name in lower case, with `-` for spaces |
| Exit `2`, `stopped at node 1 of 3: …` | A node's deploy failed; later nodes kept the old revision | Read the node's error with `forge stacks show`, fix it and apply again; nodes already live are skipped |
| Exit `7` | A rollout of this stack is already running | Wait for it (`forge jobs get <id>`), then apply again |
| Plan note `services are reachable from other nodes only through published ports; …` | The cluster has no cluster network | [Create the cluster network](/docs/guides/cluster-network/) |
| Plan note `<node>: cluster network not applied yet; …` | The node's network is not in sync | Wait for it, or **Re-sync now** on the cluster's network tab |

## Next steps

- [Deploy from CI](/docs/guides/deploy-from-ci/#deploy-a-compose-stack)
- [Compose extensions and policy](/docs/reference/compose-extensions/)
