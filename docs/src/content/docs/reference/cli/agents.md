---
title: forge agents
description: The forge agents commands that compare agent versions, update agents now or in canary batches, and set up automatic updates.
sidebar:
  order: 5
---

`forge agents` compares each node's agent version with the control plane's
and updates agents in place, verified and with automatic rollback. For the
steps and what happens on the node, see [Update agents](/docs/guides/update-agents/);
for the model, see [Agents](/docs/concepts/agents/#rollouts-and-automatic-updates).
Every command takes the [global flags](/docs/reference/cli/#global-flags).

:::note
Self-update supports Linux nodes with systemd, on `x86_64` or `aarch64`.
Windows nodes are not supported: run the installer again to update them (see
[Add nodes](/docs/guides/add-nodes/)).
:::

## Commands

| Command | What it does |
| --- | --- |
| [`forge agents versions`](#forge-agents-versions) | Shows each node's agent version against the control plane's |
| [`forge agents update`](#forge-agents-update) | Updates agents to the control plane's version, at once or in canary batches |
| [`forge agents auto-update`](#forge-agents-auto-update) | Shows the automatic update setting, or changes it |

### forge agents versions

Shows the agent version of every node you can see, against the control
plane's version. Needs `view` on each node; revoked nodes are left out.

**Synopsis**

```text
forge agents versions [--outdated]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--outdated` | flag | off | no | Only nodes whose agent version differs from the control plane's | `--outdated` |

**Examples**

```sh
# Every node's agent version
forge agents versions

# Only the nodes that need an update
forge agents versions --outdated

# Host names of the outdated nodes that self-update can reach
forge -o json agents versions | jq -r '.result.nodes[] | select(.updatable) | .hostname'
```

**Output**

The table goes to stdout; the summary line goes to stderr:

```text
NODE     STATUS  OS     AGENT                      OUTDATED
node-01  online  linux  0.1.1-dev.20261113.214755  false
node-02  online  linux  0.1.1-dev.20261113.214755  false
control plane: 0.1.1-dev.20261113.214755 · 0 outdated
```

With `-o json`, the server's whole answer. `target` is the control plane's
version, `outdated` counts the outdated nodes, and `updatable` is `true` for
an outdated Linux node. With `-o json`, `--outdated` does not filter `nodes`:

```json
{
  "result": {
    "nodes": [
      {
        "agent_version": "0.1.1-dev.20261113.214755",
        "arch": "x86_64",
        "display_name": null,
        "hostname": "node-01",
        "id": "01900000-0000-7000-8000-000000000004",
        "os": "linux",
        "outdated": false,
        "status": "online",
        "updatable": false
      },
      …
    ],
    "outdated": 0,
    "target": "0.1.1-dev.20261113.214755"
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: success.
- `3`: not signed in.
- `9`: the server cannot be reached.

**See also**: [`forge agents update`](#forge-agents-update),
[Update agents](/docs/guides/update-agents/#verify).

### forge agents update

Updates the agent on the named nodes, or on every outdated online Linux
node, to the control plane's version. Needs `server.command` on each node
(**Operator** or higher), which requires an MFA-verified session. The CLI
asks you to type `yes` unless you pass `--yes`.

**Synopsis**

```text
forge agents update [--all] [--canary <N>] [--wait] [--wait-timeout-secs <WAIT_TIMEOUT_SECS>] [--yes] [NODES]...
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<nodes>` | node ids, host names or display names | — | yes, unless `--all` | The nodes to update, separated by spaces. Cannot be combined with `--all` | `web-01 web-02` |
| `--all` | flag | off | no | Every outdated online Linux node, chosen by the server | `--all` |
| `--canary <N>` | integer (1 to 100) | off | no | Rolls out in batches of `N` nodes as one `agent.rollout` job. See below | `--canary 2` |
| `--wait` | flag | off | no | Waits until the update jobs, or the rollout, finish | `--wait` |
| `--wait-timeout-secs <WAIT_TIMEOUT_SECS>` | integer (seconds) | `900` | no | How long `--wait` waits. The jobs keep running after a timeout | `--wait-timeout-secs 3600` |
| `--yes` | flag | off | no | Skips the confirmation prompt. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

Without `--canary`, each node gets its own `agent.update` job at once. A
node Forge cannot update (no `server.command`, no MFA-verified session,
revoked, an update already queued or running) is listed under `refused`; the
others go ahead. One request takes at most 500 nodes.

With `--canary <N>`:

- The first batch is the canary. The next batch starts only when every node
  of the last one is back on the target version or was skipped. Forge skips
  a node that is offline until the end of its batch or cannot self-update.
  The first real failure stops the rollout, and later nodes are not touched.
- Named nodes are updated in the order you give.
- Forge checks `server.command` on every node before it starts and refuses
  the whole rollout if one is missing.
- The rollout job sits on the platform, so you also need `job.create` and
  `view` on the platform: a role bound on the platform, such as
  **Platform admin**.
- Only one rollout runs at a time, and the server's
  [`jobs.concurrency`](/docs/reference/configuration/) must be `2` or more.
- A rollout can take up to 30 minutes per batch. Raise
  `--wait-timeout-secs` when you combine it with `--wait`.

:::caution
An update restarts `forge-agent` and `forge-agent-helper` on the node. Jobs
running on that node at that moment can fail; run them again.
:::

**Examples**

```sh
# Update two nodes after a confirmation prompt
forge agents update <node> <node>

# Update every outdated node and wait for the result
forge agents update --all --wait --yes

# Roll out to every outdated node, one node at a time, and wait up to two hours
forge agents update --all --canary 1 --wait --wait-timeout-secs 7200 --yes
```

**Output**

Not captured on a live system, because it restarts agents. The shapes come
from the server's schema; the default output prints the same objects
without the wrapper.

Without `--canary` and `--wait`, the queued jobs:

```json
{
  "result": {
    "target": "<control-plane-version>",
    "jobs": [
      { "server_id": "<node-id>", "job_id": "<job-id>" }
    ],
    "refused": [
      { "server_id": "<node-id>", "error": "another update of this node is queued or running (job <job-id>)" }
    ]
  },
  "schema": "forge.cli/v1"
}
```

With `--wait`, the final status of each job. `error` is the job's last error,
or `null`:

```json
{
  "result": {
    "target": "<control-plane-version>",
    "results": [
      { "job_id": "<job-id>", "status": "succeeded", "error": null }
    ],
    "refused": []
  },
  "schema": "forge.cli/v1"
}
```

With `--canary`, the rollout job and its size. With `--canary --wait`, the
output is `{"target": …, "rollout": <job>}` with the finished job:

```json
{
  "result": {
    "job": { "id": "<job-id>", "kind": "agent.rollout", "status": "queued", … },
    "target": "<control-plane-version>",
    "nodes": 6,
    "batches": 3
  },
  "schema": "forge.cli/v1"
}
```

Exit codes:

- `0`: the jobs or the rollout were queued; with `--wait`, all of them
  succeeded. Refused nodes alone do not change the exit code.
- `1`: you did not type `yes` at the prompt (`cancelled`).
- `2`: no nodes and no `--all`, nodes together with `--all`, no `--yes` in
  non-interactive use, `--canary` outside 1 to 100, more than 500 nodes, or
  an ambiguous name.
- `3`: not signed in.
- `4`: with `--canary`, you lack `server.command` on at least one node, or
  `job.create` on the platform.
- `5`: with `--canary`, an MFA-verified session is needed and the CLI
  cannot prompt for a code (run [`forge auth step-up`](/docs/reference/cli/auth/#forge-auth-step-up) first).
- `6`: a named node does not exist or you cannot see it.
- `7`: with `--canary`, another rollout is queued or running, no outdated
  online Linux node is left, or `jobs.concurrency` is below `2`.
- `8`: `--wait` timed out; the jobs keep running.
- `10`: with `--wait`, at least one update did not succeed
  (`partial_failure`), or the rollout stopped (`job_failed`,
  `job_cancelled`).

**See also**: [Update agents](/docs/guides/update-agents/),
[`forge agents versions`](#forge-agents-versions).

### forge agents auto-update

Without flags, shows the automatic agent update setting. With flags, changes
it. Showing needs `view` on the workspace. Changing needs `system.update` on
the platform (only **Platform admin** has it) and an MFA-verified session.
Changes are audited as `agent.auto_update.update`. The CLI asks you to type
`yes` before it saves unless you pass `--yes`.

**Synopsis**

```text
forge agents auto-update [--enable | --disable] [--days <DAYS>] [--start <START>] [--end <END>] [--batch <BATCH>] [--yes]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--enable` | flag | off | no | Turns automatic updates on. Cannot be combined with `--disable` | `--enable` |
| `--disable` | flag | off | no | Turns automatic updates off | `--disable` |
| `--days <DAYS>` | list of `mon`, `tue`, `wed`, `thu`, `fri`, `sat`, `sun` | current value, else every day | no | Weekdays on which the window opens. `--days ""` means every day | `--days sat,sun` |
| `--start <START>` | time `HH:MM`, UTC | current value, else `02:00` | no | Window start | `--start 02:00` |
| `--end <END>` | time `HH:MM`, UTC | current value, else `05:00` | no | Window end, exclusive. It may be past midnight; the window then belongs to the day it started. Must differ from `--start` | `--end 05:00` |
| `--batch <BATCH>` | integer (1 to 100) | current value, else `1` | no | Nodes per batch; the first batch is the canary | `--batch 2` |
| `--yes` | flag | off | no | Skips the confirmation prompt. Required with `--non-interactive` or when stdin is not a terminal | `--yes` |

Flags you leave out keep their current value; the CLI reads the setting
first and sends it back with your changes.

:::note
On a server where the setting was never saved, it reads back with an empty
`start` and `end` and `batch` `0`. When you change it, the CLI then uses
`02:00`, `05:00` and `1` for the flags you leave out, and every day for
`--days`.
:::

How automatic updates run:

- Inside the window, at most every five minutes, Forge starts one rollout of
  every outdated online Linux node, in batches of `--batch`. It tries once
  per control-plane version; after that, a person looks before the next
  version.
- Rollouts run as **you**, the person who saved the setting with `--enable`.
  You need `server.command` on every node they update, now and later.
- The MFA of the session that saved the setting counts for 90 days
  (`expires_at`). After that, or when your account is disabled or deleted,
  automatic rollouts stop. Run the command again with `--enable` from an
  MFA-verified session to renew it; saving again also clears `last_error`.

:::caution
Automatic updates restart `forge-agent` and `forge-agent-helper` on your
nodes while nobody watches. Pick a window in which restarts are acceptable,
and keep the batch small so that the canary catches a bad release.
:::

**Examples**

```sh
# Show the setting
forge agents auto-update

# Turn automatic updates on, Saturdays and Sundays 02:00 to 05:00 UTC, one node per batch
forge agents auto-update --enable --days sat,sun --start 02:00 --end 05:00 --batch 1

# Move the window past midnight, keeping everything else
forge agents auto-update --start 23:00 --end 01:00 --yes

# Turn automatic updates off
forge agents auto-update --disable --yes
```

**Output**

Showing, captured on a server where the setting was never saved:

```text
{
  "batch": 0,
  "days": [],
  "enabled": false,
  "enabled_at": null,
  "enabled_by": null,
  "end": "",
  "expires_at": null,
  "last_error": null,
  "last_job": null,
  "last_target": null,
  "start": "",
  "timezone": "UTC"
}
```

```json
{
  "result": {
    "batch": 0,
    "days": [],
    "enabled": false,
    "enabled_at": null,
    "enabled_by": null,
    "end": "",
    "expires_at": null,
    "last_error": null,
    "last_job": null,
    "last_target": null,
    "start": "",
    "timezone": "UTC"
  },
  "schema": "forge.cli/v1"
}
```

A change prints the saved setting in the same shape. Shape from the server's
schema; not captured on a live system:

```json
{
  "result": {
    "enabled": true,
    "days": ["sat", "sun"],
    "start": "02:00",
    "end": "05:00",
    "batch": 1,
    "enabled_by": "user:<user-id>",
    "enabled_at": 1794590525506,
    "expires_at": 1802366525506,
    "last_target": null,
    "last_job": null,
    "last_error": null,
    "timezone": "UTC"
  },
  "schema": "forge.cli/v1"
}
```

| Field | Meaning |
| --- | --- |
| `enabled_by` | The principal that rollouts run as; `null` when off |
| `enabled_at`, `expires_at` | When the setting was saved, and when its MFA stops counting (90 days later), in epoch milliseconds |
| `last_target` | The version the last automatic rollout was started for |
| `last_job` | The id of that rollout job |
| `last_error` | Why the last automatic attempt was refused or failed, or `null` |

Exit codes:

- `0`: success.
- `1`: you did not type `yes` at the prompt (`cancelled`).
- `2`: `--enable` with `--disable`, an unknown day, a time that is not
  `HH:MM`, a window with equal start and end, a batch outside 1 to 100, or
  no `--yes` in non-interactive use.
- `3`: not signed in.
- `4`: you lack `system.update` on the platform.
- `5`: the session is not MFA-verified (`step_up_required`).

**See also**:
[Update agents automatically](/docs/guides/update-agents/#update-agents-automatically),
[`forge agents update`](#forge-agents-update).
