---
title: Update agents
description: Update the agent and helper on Linux nodes to the control plane's version, from the console or the forge CLI.
sidebar:
  order: 4
---

This guide shows you how to bring the agents on your nodes up to the version
of the control plane: one node at a time, in batches with a canary first, or
automatically inside a maintenance window. Each update is verified, and a node
rolls back on its own if the new agent does not come back.

## Before you begin

- You need the `server.command` action on each node. The built-in roles
  **Platform admin**, **Admin** and **Operator** have it.
- `server.command` requires an MFA-verified sign-in. See [MFA](/docs/guides/mfa/).
- Self-update supports Linux nodes with systemd, on `x86_64` or `aarch64`.
- Windows nodes are not supported. To update a Windows agent, run the
  installer again (see [Add nodes](/docs/guides/add-nodes/)).
- The node must be online. It downloads the new binaries from the control
  plane's agent download URL with `curl`, so `curl` must be installed and the
  control plane must be reachable from the node.
- The node's `agent.toml` must allow the `update` command. New installs allow
  it. Re-running the node installer adds it to older agents (see
  [Configuration](/docs/reference/configuration/#agent-agenttoml)).

:::caution
An update restarts `forge-agent` and `forge-agent-helper` on the node. Jobs
running on that node at that moment can fail; steps are idempotent, so you
can run them again.
:::

## Update nodes now

### From the console

1. Open **Administration → Forge updates** and find the **Agents** panel.
   It lists every node you can see, its **Status**, **Platform** and
   **Agent** version. Outdated agents carry an **outdated** badge. Use the
   search box to filter by node name, platform, version or status; long lists
   are paged (**Rows** 10, 20 or 50). **Update agents** always covers every
   outdated online Linux node, not only the visible page.
2. Select **Update** in the node's row.
3. Forge starts an `agent.update` job for the node. Follow it on the
   **Operations → Jobs** page.

The **Update** button is disabled for Windows nodes and offline nodes. Hover
over it to see why.

:::note
An agent is **outdated** only when it runs an older version than the control
plane. Forge orders a release candidate or dev build of `X.Y.Z`
(`1.0.0-rc-01`, `1.0.0-dev.<stamp>`) below `X.Y.Z`. An agent on a newer
version than the control plane is **ahead**: updates, rollouts and automatic
updates leave it alone, so they never downgrade an agent.
::: To update several nodes from the console, use
**Update agents**, which starts a [rollout](#roll-out-in-batches-canary).

### From the CLI

1. List agent versions:

   ```sh
   forge agents versions
   forge agents versions --outdated
   ```

2. Update named nodes, or every outdated online Linux node:

   ```sh
   forge agents update <node-name> <other-node>
   forge agents update --all
   ```

   Forge asks for confirmation. Pass `--yes` to skip it in scripts.

3. To wait until the jobs finish, add `--wait`:

   ```sh
   forge agents update --all --wait --yes
   ```

   The command exits with code `10` when one or more updates did not
   succeed. `--wait-timeout-secs` (default `900`) limits the wait; the jobs
   keep running after a timeout.

## What happens on the node

The `agent.update` job has two steps:

1. **send**: Forge sends the node a signed update command with the target
   version and the SHA-256 of the agent and helper for the node's
   architecture. The target version is the version of the agent binaries the
   control plane hands out, from `MANIFEST.json` in its
   [`agent_dist_dir`](/docs/reference/configuration/), not the server's own
   version. The root helper then:
   1. takes the update lock (`/var/lib/forge-agent-helper/update.lock`). A
      second update while one runs is refused as busy and retried later;
   2. does nothing when the installed binaries already have these hashes,
      and refuses a version older than its own;
   3. downloads both binaries into a new folder under
      `/var/lib/forge-agent-helper`, which only root can open, and checks
      the checksums;
   4. runs `forge-agent --version` and `forge-agent-helper --version` on the
      new binaries. Both must print the target version;
   5. writes the intent marker `/var/lib/forge-agent-helper/update-intent.json`,
      then for each binary writes `<binary>.new` next to it in
      `/usr/local/bin`, flushes it to disk, checks its hash again, keeps the
      current binary as `<binary>.prev` and renames `.new` over it;
   6. writes the pending marker
      `/var/lib/forge-agent-helper/update-pending.json`, with a copy for the
      agent at `/var/lib/forge-agent/update-pending.json`, and removes the
      intent marker. It restarts both services about three seconds later.
2. **await**: Forge waits for the node to report the target version again,
   then for the node's facts to show the helper on that version too.

After its first good heartbeat, the new agent asks the new helper to confirm
the update. The helper confirms only for its own version, and not while an
update runs. It then removes the marker (kept as `update-last.json`) and the
`*.prev` backups. A node that already runs the target version, or a newer
one, is skipped.

If the node crashes or loses power during the swap, the helper finds the
intent marker when it starts. When both binaries are already the new ones,
it finishes the update. Otherwise it puts the `*.prev` binaries back. Either
way it restarts both services, and you can run the update again.

For background, see [Agents](/docs/concepts/agents/) and [Jobs](/docs/concepts/jobs/).

## Verify

- In the **Agents** panel, the node shows the control plane's version with a
  check mark.
- `forge agents versions --outdated` no longer lists the node.
- The `agent.update` job has the status `succeeded`.
- On the node, `forge-agent --version` prints the new version.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| The job fails with "self-update is available for Linux nodes" | The node runs Windows | Reinstall the agent with the node installer |
| The job retries with "the node is offline" | The node is not connected | Bring the node online; the job retries up to 3 times, then fails. Update it again once it is online |
| The job fails with "this control plane has no agent binaries for linux/`<arch>`" | The control plane has no agent build for that architecture, or the build does not match `MANIFEST.json` in `agent_dist_dir` (the server log says `agent binary does not match MANIFEST.json`) | Add the binaries for that architecture to the control plane's agent distribution, write the manifest again, and restart `forge-server` |
| The job fails with "this update was for `<version>`, but this control plane now hands out agents `<other>`" | The agent binaries changed after the update was queued, for example a server update restarted the control plane | Start a new update or rollout |
| "the node refused the update: … needs systemd" | The node does not run systemd | Reinstall the agent instead |
| "the node refused the update: … needs curl" | `curl` is not installed on the node | Install `curl`, then update again |
| "the node refused the update: checksum mismatch" | The downloaded binary is not the one the control plane signed for, often a proxy or a changed file | Check what serves `/agent/download/` to the node. Nothing was installed |
| "the node refused the update: download failed" | `curl` is missing, the URL redirects, or the control plane is unreachable | Install `curl`; make the node reach the control plane URL directly. Redirects are not followed |
| "the node refused the update" with a command-policy error | `update` is not in `allowed_commands` | Re-run the node installer, or add `"update"` to `/var/lib/forge-agent/agent.toml` |
| `403` or an MFA prompt | No `server.command` on the node, or the sign-in is not MFA-verified | Get a role with `server.command`; sign in with MFA |
| The job retries with "another agent update is running on this node" | The node's helper is installing another update | Nothing to do. The job tries again with a backoff |
| "the node refused the update: … is older than …" | The control plane offers an older agent than the node runs | The node is ahead and keeps its version. Rollouts skip it |
| "the new forge-agent-helper reports …, expected …" | The new helper does not start or reports another version. Nothing was installed | Check the agent binaries in `agent_dist_dir` and their `MANIFEST.json` |
| "the agent runs `<version>`, but the node's helper still runs `<old>`" | The helper binary was not replaced | Check `journalctl -u forge-agent-helper` on the node, then update again |
| "the agent runs `<version>`, but the node's helper does not answer" | The new helper does not start, so the node has no rollback watchdog | Check `journalctl -u forge-agent-helper` on the node. Reinstall the agent if the helper does not start |
| `409` "another update of this node is queued or running (job `<id>`)" | The node already has an update in flight, often from a rollout. Forge sends a node one update at a time | Wait for that job to finish, or cancel it, then update again |
| The job fails with "the node did not come back on `<version>`" | The new agent did not reconnect | Wait a few minutes: the node rolls back on its own (see below) |
| The job fails with "the node is back on `<version>`" | The new agent did not stay up, and the node's helper restored the previous one | The node runs the previous agent again. Read the rollback reason on the node (see below) |

### Automatic rollback

The helper runs a watchdog while
`/var/lib/forge-agent-helper/update-pending.json` exists. It restores the
`*.prev` binaries and restarts both services when one of these happens:

- the new agent has not confirmed within 5 minutes of the swap,
- systemd has restarted the agent 3 times since the swap (an agent crash
  loop; the helper reads the unit's `NRestarts` every 5 seconds), or
- the helper has started more than 3 extra times with the marker present
  (a helper crash loop).

A crashing agent is rolled back in well under a minute. An agent that runs
but cannot reach the control plane is rolled back at the 5 minute mark.

The `agent.update` job fails as soon as the node reports a version other
than the target again, two minutes after the update was sent at the
earliest. It does not wait out its own 6 minute limit.

After a rollback the marker is renamed to
`/var/lib/forge-agent-helper/update-rolled-back.json`, and the helper logs
`self-update rollback (<reason>)`. Read it with:

```sh
journalctl -u forge-agent-helper -u forge-agent --since "1 hour ago"
sudo cat /var/lib/forge-agent-helper/update-rolled-back.json
```

Fix the cause, then run the update again. A new update request for a node
whose last update failed starts a new `agent.update` job.

## Roll out in batches (canary)

A rollout updates many nodes batch by batch. The first batch is the canary.
The next batch starts only when every node of the current batch is back on
the target version or was skipped. Forge skips a node that cannot take the
update: offline until the end of its batch, not Linux, or a node that
refuses it as unsupported (no systemd, no `curl`, or `update` not allowed).
Skipped nodes are listed with the reason, and the rollout carries on. The
first real failure, such as a new agent that does not come back, stops the
rollout, and nodes in later batches are not touched. For background, see
[Agents](/docs/concepts/agents/#rollouts-and-automatic-updates).

### Before you begin

- You need `server.command` (MFA) on **every** node in the rollout. Forge
  checks all of them before it starts, and refuses the whole rollout with
  `403` if one is missing.
- You need a role bound on the platform (scope `platform:root`), such as
  **Platform admin**, or **Operator** bound on the platform. The rollout job
  lists nodes from every workspace, so it sits on the platform: `job.create`
  and `view` on the platform start it, and `job.cancel` on the platform
  cancels it. A role bound only on a workspace cannot start, see or cancel a
  rollout. Update those nodes one at a time instead.
- Only one rollout runs at a time. Forge refuses a new one with `409` while
  another is queued or running. A rollout holds a job worker while it waits
  for its batches, so this keeps workers free for other jobs.
- The control plane must run at least two jobs at once:
  [`jobs.concurrency`](/docs/reference/configuration/) must be `2` or more. The
  default is `8`. The rollout job waits while the update jobs it started run.
- Each node must meet the prerequisites in
  [Before you begin](#before-you-begin) above.

### Steps

#### From the console

1. Open **Administration → Forge updates** and find the **Agents** panel.
2. Select **Update agents (N)**, where N is the number of nodes that will
   update. The **Update agents** dialog opens.
3. Check the three groups:
   - **Will update**: outdated nodes that are online and can update
     themselves. All are ticked at first; clear the ones you want to leave
     out. **not checked** marks an older agent that does not report whether
     it can update itself; it is updated, and it refuses if it cannot.
     **until recreated** marks a node in a container whose agent comes from
     the image: the update lasts until the container is recreated, so
     update the image too.
   - **Needs reinstall**: nodes that cannot self-update, with the reason.
     Fix the reason or run the node installer again (see the table below).
   - **Offline (skipped)**: outdated nodes that are not connected. Start
     another rollout once they are online.
4. Enter the **Batch size**: the number of nodes per batch, from 1 to 100.
   The first batch is the canary, so `1` tests the release on one node first.
5. Select **Start rollout**. Forge starts one `agent.rollout` job.

The dialog, and then the **Agents** panel, show the rollout's progress:
for example `batch 1 of 3`, one chip per node with its result, and the error that
stopped the rollout. Select **Details** to open the job. You can close the
dialog; the rollout keeps running.

#### From the CLI

1. Start a rollout over every outdated online Linux node, with one node in
   the first batch:

   ```sh
   forge agents update --all --canary 1
   ```

   Or name the nodes. Forge updates them in the order you give, so put the
   canary first:

   ```sh
   forge agents update <canary-node> <node-2> <node-3> --canary 1
   ```

2. To wait for the whole rollout, add `--wait`. One batch can take up to
   `[update] agent_batch_timeout_secs` (30 minutes by default) plus 10
   minutes, so raise the wait limit:

   ```sh
   forge agents update --all --canary 5 --wait --wait-timeout-secs 7200 --yes
   ```

   The command exits with code `10` when the rollout stopped, and `8` when
   the wait timed out. The rollout keeps running after a timeout.

#### With the API

```sh
curl -s -X POST -H "Authorization: Bearer $FORGE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"server_ids": [], "batch": 2}' \
  "https://<forge-host>/api/v1/agents/rollouts"
```

An empty `server_ids` selects every outdated online Linux node. See
[REST API: agent updates](/docs/reference/rest-api/#agent-updates).

#### Fix a node that needs a reinstall

The **Agents** panel marks such a node **needs reinstall**; hover over the
badge to see why.

| Reason | Fix |
| --- | --- |
| Self-update is for Linux nodes | Run the Windows node installer again |
| No agent build for this platform | Add the agent binaries for the node's architecture to `agent_dist_dir`, write `MANIFEST.json`, and restart `forge-server` |
| No systemd on this node | Run the node installer again |
| curl is not installed | Install `curl` on the node |
| `/usr/local/bin` is not writable | Make it writable for root (a read-only root file system or mount), or reinstall |
| Not enough free disk space | Free at least twice the size of the agent and helper on the file system of `/usr/local/bin` |

The node reports its state when it starts a new agent version, and the
control plane asks again once a day and when `forge-server` restarts.

### Verify

- The `agent.rollout` job has the resource `platform:root` and no
  workspace. Each `agent.update` job it starts belongs to its node's
  workspace.
- The `agent.rollout` job has the status `succeeded`. In its state,
  `done_batches` equals `batches`, and every entry in `nodes` has the
  `outcome` `succeeded` or `skipped`. `skipped` counts the skipped nodes;
  each skipped entry has a `skip_reason` (`offline`, `unsupported`,
  `downgrade` or `busy`).
- `forge agents versions --outdated` no longer lists the nodes.
- The audit log has an `agent.rollout` entry with the node count, batch size
  and target version.

### Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| The rollout fails with "stopped in batch k of n: node `<id>` failed (…); later nodes were not touched" | That node's update failed. Nothing after it ran | Open the node's `agent.update` job for the reason and fix it (see [Troubleshooting](#troubleshooting) above). Then start a new rollout: it updates the nodes that are still outdated, including the one that failed |
| "batch k of n did not finish in `<m>` minutes; later nodes were not touched" | A node of the batch did not come back in time | Check that node. Its own update job can still finish or roll back. Then start a new rollout |
| A node is listed as `skipped` with `skip_reason` `offline` | The node was not connected before the end of its batch | Bring it online and start a new rollout, which updates the nodes that are still outdated |
| A node is listed as `skipped` with `skip_reason` `unsupported` | The node cannot self-update: no systemd, no `curl`, or `update` not in `allowed_commands` | Reinstall the agent with the node installer, or fix the cause named in `reason` |
| A queued update fails with "the rollout was cancelled; this node was not updated" | You cancelled the rollout | Nothing to do. Start a new rollout when you are ready |
| Every node of the first batch was skipped | The canary nodes were offline or cannot self-update, so nothing was tested yet | The next batch is then the canary. To test on a chosen node first, start a rollout with that node alone |
| "stopped in batch k of n: node `<id>` was refused (…)" | Forge could not queue that node's update job | Read the reason in the message, fix it and start a new rollout |
| "… was refused (… another update of this node is queued or running …)" | That node already had an update in flight, for example one started from its row | Wait for that update to finish, then start a new rollout |
| `409` "rollout `<id>` is still queued or running" | Another rollout has not finished. The refusal is in the audit log as a denied `agent.rollout` | Wait for it, or cancel it on the **Jobs** page, then start yours |
| `409` "no outdated online Linux node to update" | Every selected node is current, offline or not Linux | Nothing to do, or bring the nodes online first |
| `409` "rollouts need [jobs] concurrency of at least 2" | `jobs.concurrency` is `1` | Set `jobs.concurrency` to `2` or more and restart `forge-server` |
| `400` "batch: 1 to 100 nodes" or "at most 500 nodes per rollout" | The batch size or the node list is out of range | Use a batch of 1 to 100 and at most 500 nodes. Split larger fleets into several rollouts |
| `403` before anything starts | You lack `server.command` on at least one node, or your session is not MFA-verified. The message says how many nodes were refused | Get the role on every node, or leave those nodes out. Sign in with MFA |
| `403` for `job.create` or `view` | Your role is bound on a workspace, not on the platform | Ask a platform administrator to start the rollout, or update the nodes one at a time |

## Update agents together with the server

When you update the Forge server from **Administration → Forge updates** or
with `forge system apply`, the agents follow by default (`--agents rollout`).
After the new server serves and the agents reconnect, it starts one canary
rollout of every outdated online Linux agent to its own version, as you. You
need `server.command` on the nodes, as for any rollout. Set the batch size
with `--batch`. Windows agents still need a reinstall.

1. Run `forge system releases` and pick a version.
2. Run `forge system apply --version <version> --batch 2 --yes`.
3. Follow the rollout with `forge jobs list`.

The audit log records `system.update.agents` with the rollout job, or with
the reason when it was refused. To leave the agents alone, pass
`--agents none` and roll them out later as described above.

## Update agents automatically

Automatic updates start a rollout for you when the control plane has a newer
agent version than your nodes. They run only inside a weekly maintenance
window in UTC, and they are off until you turn them on.

### Before you begin

- The setting covers every node, so you need the `system.update` action on
  the platform. Only the built-in role **Platform admin** has it.
- You need an MFA-verified session. Without one, saving fails with `403`
  `step_up_required`.
- Automatic rollouts run as **you**, with the MFA of the session that saved
  the setting. You need `server.command` on every node they update, now and
  later. If you lose it, the automatic rollout is refused and audited.
- Your account must stay enabled. Automatic rollouts stop when it is
  disabled or deleted.
- The MFA you saved with is good for 90 days (`expires_at` in the API).
  After that, automatic rollouts stop until someone saves the setting again
  from an MFA-verified session.
- [`jobs.concurrency`](/docs/reference/configuration/) must be `2` or more, as for
  any rollout.

:::caution
Automatic updates restart `forge-agent` and `forge-agent-helper` on your
nodes while nobody watches. Pick a window in which restarts are acceptable,
and keep the batch small so that the canary catches a bad release.
:::

### Steps

1. Open **Administration → Forge updates** and find the **Automatic updates** panel.
2. Turn on **Update agents automatically**.
3. Under **Days**, pick the weekdays (**Mon** to **Sun**). Pick none to allow
   every day.
4. Set **Start** and **End** in UTC, as `HH:MM`. The end may be past
   midnight, and the window belongs to the day it starts: Saturday `23:00` to
   `01:00` runs from Saturday 23:00 to Sunday 01:00.
5. Set the **Batch size**, from 1 to 100. The first batch is the canary.
6. Select **Save**. **Reset** discards changes you have not saved.

Saving needs `system.update` on the platform, which only **Platform admin**
has. Saving again renews the 90 days and makes you the person automatic
rollouts run as.

To do the same with the API:

```sh
curl -s -X PUT -H "Authorization: Bearer $FORGE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"enabled": true, "days": ["sat", "sun"], "start": "02:00", "end": "05:00", "batch": 1}' \
  "https://<forge-host>/api/v1/agents/auto-update"
```

To turn automatic updates off, clear **Update agents automatically** and
select **Save**, or send `"enabled": false`.

Forge checks every 5 minutes. When automatic updates are on, the time is
inside the window, nodes are behind and no rollout is running, Forge starts
one rollout over every outdated online Linux node. Before it starts, Forge
checks that the person in `enabled_by` is still enabled and that their saved
MFA is at most 90 days old.

Forge starts one rollout per version. Once a rollout for a version started,
Forge does not start another for that version, even if it failed. Look at
the failure, then [start a rollout](#roll-out-in-batches-canary) yourself.
When a rollout cannot start at all, for example because a node was refused,
Forge does not count it and tries again on the next check inside the window.

### Verify

- The **Automatic updates** panel shows the **on** badge and you under
  **Turned on by**. `GET /api/v1/agents/auto-update` returns
  `"enabled": true`, your principal (`user:<id>`) in `enabled_by`, the save
  time in `enabled_at` and the end of the 90 days in `expires_at` (epoch ms).
- After the next window with a new version, **Last run** shows that version
  and a link to the rollout job (`last_target` and `last_job` in the API).
  Select the link to follow the rollout.
- The audit log has `agent.auto_update.update` for your change, and
  `agent.auto_update.start` with the actor `system:forge` when a rollout
  starts. Every automatic entry names the person it runs as in `details.as`.
- When a rollout cannot start, the audit log has one of these:

  | Action | Outcome | When |
  | --- | --- | --- |
  | `agent.auto_update.refused` | `denied` | The person in `enabled_by` is disabled or deleted, or the saved MFA is older than 90 days |
  | `agent.auto_update.node_refused` | `denied` | One entry per node the person may not command (`details.server_id`) |
  | `agent.auto_update.failed` | `denied` or `failure` | Authorization refused the rollout (`denied`), or it failed for another reason (`failure`) |

  `forge audit list --outcome denied` and `--outcome failure` find them.

### Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| Saving fails with `403` `step_up_required` | The server requires MFA and your session is not MFA-verified | Confirm with your authenticator code and save again |
| Saving fails with `403` | You lack `system.update` on the platform | Ask a platform administrator |
| Saving fails with `400` "start and end: HH:MM (UTC)" | A time is not in `HH:MM` form | Use times like `02:00` |
| Saving fails with `400` "the window needs a length" | **Start** and **End** are equal | Pick different times |
| No rollout starts | The time is outside the window (UTC, not your local time), no node is behind, a rollout is already queued or running, or Forge already started a rollout for this version | Check the window in UTC, and compare `last_target` with the version in `GET /api/v1/agents/versions` |
| `last_error` says "… is disabled" or "… no longer exists", with `agent.auto_update.refused` in the audit log | The person in `enabled_by` can no longer act | Have another platform administrator save the setting from an MFA-verified session |
| `last_error` says "the setting was saved more than 90 days ago" | The saved MFA expired (`expires_at`) | Save the setting again from an MFA-verified session |
| The panel shows "Last automatic rollout: …" (`last_error` in the API), and the audit log has `agent.auto_update.failed` | The rollout could not start, for example because the person in `enabled_by` lost `server.command` on a node (`agent.auto_update.node_refused` names the nodes) | Fix the cause. Forge tries again on the next check inside the window, or start a rollout by hand. To make someone else the owner, have them save the settings |
| The automatic rollout job failed | A node's update failed. Later batches were not touched | See the [rollout troubleshooting](#troubleshooting-1) table |
| `PUT /api/v1/settings/agents.auto_update` fails with `400` | Keys that start with `agents.` have their own endpoints | Use **Administration → Forge updates** or `PUT /api/v1/agents/auto-update` |

## Next steps

- [Agents: rollouts and automatic updates](/docs/concepts/agents/#rollouts-and-automatic-updates)
- [REST API: agent updates](/docs/reference/rest-api/#agent-updates)
- [Command line](/docs/reference/cli/)
- [Cedar schema](/docs/reference/cedar-schema/)
