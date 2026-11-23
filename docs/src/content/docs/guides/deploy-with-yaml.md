---
title: Deploy a service from YAML
description: Plan, apply, follow, approve and roll back a release of a service from a forge.deploy.yaml document, in the CLI or the console.
sidebar:
  order: 7
---

This guide shows you how to describe a release of a service in a
`forge.deploy.yaml`, check what it will do, roll it out without downtime,
approve gated steps and roll back. For how rollouts work, see
[Deployments and releases](/docs/concepts/deployments/).

## Before you begin

- **Permissions.** `deploy` on the service, or on its environment when the
  document creates the service. Roles `platform_admin`, `workspace_admin`,
  `operator` and `developer` hold it. A placement that adds or removes nodes
  also needs `server.manage` on each of them. Approving needs
  `deploy.approve` (`platform_admin`, `workspace_admin`, `operator`) and
  another person than the one who started the deployment.
- **Nodes.** Linux nodes with a connected agent; Debian and Ubuntu are
  tested. Containers need Docker Engine (Forge installs it on first deploy
  when it is missing). Windows nodes are not supported.
- **An environment** for the service, for example `staging`. See
  [Services, environments and services](/docs/concepts/projects-environments-services/).
- **An endpoint** that routes to the service, for any strategy that runs two
  copies side by side (`rolling` start-first, `blue-green`, `canary`). Edge
  nodes must reach the service's nodes on TCP 20000–29999. Without an
  endpoint, use `strategy.replacement: stop-first` or `type: recreate`. See
  [Sites and DNS](/docs/guides/sites-and-dns/).
- **The CLI**, signed in. See [CLI reference](/docs/reference/cli/).

## Steps

1. Write `forge.deploy.yaml` next to your code:

   ```yaml
   apiVersion: forge/v1
   kind: Deployment
   metadata:
     name: web
     environment: staging
   spec:
     runtime: container
     artifact:
       image: ghcr.io/<org>/web:1.4.2
     template:
       ports: ["8080:80"]
     placement:
       nodes: [<node-1>, <node-2>]
     strategy:
       type: rolling
     health:
       checks:
         readiness:
           - name: http
             probe: { type: http, port: 80, path: /healthz }
   ```

   Every field is listed in [Deployment YAML](/docs/reference/deployment-yaml/).
   Put secret values on the service, never in the file; list only their
   names under `spec.secrets`.

2. Check the document:

   ```sh
   forge deploy validate -f forge.deploy.yaml
   ```

3. See the plan. Nothing starts:

   ```sh
   forge deploy plan -f forge.deploy.yaml
   ```

   Read **Downtime** and the steps. If Forge refuses the plan, the message
   says why and what to change.

4. Apply it and follow the rollout:

   ```sh
   forge deploy apply -f forge.deploy.yaml --wait
   ```

   To target another environment with the same file, add
   `--environment prod`; that environment's `spec.overrides` entry is merged
   in.

5. If the deployment waits for an approval, a second person with
   `deploy.approve` approves or rejects it:

   ```sh
   forge deploy approve <deployment-id>
   forge deploy reject <deployment-id>
   ```

6. To hold or stop a rollout:

   ```sh
   forge deploy pause <deployment-id>
   forge deploy resume <deployment-id>
   forge deploy abort <deployment-id>
   ```

7. To go back to the previous release:

   ```sh
   forge deploy rollback web -n staging --wait
   ```

   Add `--to <number>` for a specific release, or use
   `forge deploy redeploy web <number>` to roll a release out again.

:::caution
A rollback restores the service's definition, artifact and environment. It
does not undo database migrations or data the newer release wrote. Keep
schema changes backward compatible.
:::

### In the console

1. Open the service page (the service, then the environment, then the
   service).
2. Select the **Releases** tab.
3. Select **Apply YAML**, paste the document and select **Plan**. Review
   **Service**, **Environment**, **Release**, **Strategy**, **Downtime**,
   **Nodes** and **What happens**.
4. Select **Apply**. The deployment opens in a drawer with its **Steps**
   and **Events**, updated live.
5. In the drawer, use **Approve** or **Reject** when the deployment waits
   for an approval (shown only to people who may approve), and **Pause**,
   **Resume** or **Abort** while it runs.
6. To go back, select **Roll back** at the top of the tab, or **Roll out
   again** on a row of **Releases**.

The tab also lists **Copies** (each node's slots, release and traffic
weight) and **Deployments**; select a deployment to open its drawer, or a
release number to see its YAML.

## Verify

- `forge deploy apply --wait` exits `0` and prints `release <n> is live`.
- `forge deploy slots web -n staging` shows the new release with weight
  `100` on every node, and **Current release** is the new number.
- `forge deploy history web -n staging` shows the deployment as
  `succeeded`.
- The service page shows its health as healthy on each node.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `… starts the new release next to the old one on other host ports, which only a Forge endpoint routes to` | Slotted strategy without an endpoint | Add an endpoint that targets the service, or set `replacement: stop-first` or `type: recreate` |
| `stop-first with one node would take every running copy down at once` | Stop-first covers every node | Add a node, lower `batchSize`, use start-first behind an endpoint, or set `strategy.allowDowntime: true` |
| `<node> offline; bring it back or change the placement` | A node of the placement is offline | Bring the agent back, or change `placement` |
| `secret \`X\` is not set on the service` | `spec.secrets` names a secret the service does not have | Set it on the service's **Environment** tab, then apply again |
| Exit `7`, `the service already has a deployment running` | One deployment per service at a time | Wait, or abort the running one |
| `… did not become ready and stable within 300s: …` | The new copy fails its readiness checks | Read the failing check in the message, fix the release, or raise `health.startupTimeout` |
| `… cannot probe copies: update its agent` | The node's agent is too old | [Update the agent](/docs/guides/update-agents/) |
| `someone other than its author approves a deployment` | You started the deployment | Ask another person with `deploy.approve` |
| A slotted copy is ready but gets no traffic | The edge cannot reach port 20000–29999 on the node | Open those ports from the edge nodes to the service's nodes |
| The deployment ends `rolled_back` | It failed after the new release took traffic | Read the error with `forge deploy status <id>`; the automatic rollback is listed in the history |

## Next steps

- [Deploy from CI](/docs/guides/deploy-from-ci/)
- [Deployment YAML](/docs/reference/deployment-yaml/)
- [`forge deploy`](/docs/reference/cli/deploy/)
- [Health checks](/docs/guides/health-checks/)
