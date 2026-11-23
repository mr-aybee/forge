---
title: Node overview, storage and connectivity
description: Live CPU, memory, disk and network charts per node, every mounted volume, inter-node connectivity checks run from the node itself, and the process view with its actions.
sidebar:
  order: 4
---

This guide shows you how to check a node's specs, live load, volumes,
network paths to its peers and running processes, and how to end a process
or change its priority.

## Before you begin

- Viewing a node needs the `view` action, which every role holds.
- **Collect facts**, **Run checks** and the **Processes** panel need
  `server.manage` on the node (**Operator**, **Admin** or **Platform admin**).
  **Collect facts** also needs `job.create`.
- Ending a process or changing its priority needs `process.manage` on the
  node (**Operator**, **Admin** or **Platform admin**) and an MFA-verified
  session. See [Multi-factor authentication](/docs/guides/mfa/).
- Live metrics, connectivity checks and processes need the node to be
  online.
- The process view and its actions work on Linux and Windows nodes.

## Open a node

Open **Nodes** and select the node; the **Cluster** filter narrows the list to
one cluster. The node page has the tabs
**Overview**, **Processes**, **Services**, **Packages**, **Containers**, **Files**,
**SSH / Terminal**, **Credentials** (when you may manage it) and **Details**
(identity, placement, agent commands, facts and recent jobs).

## Check system specs


The **System** panel on the **Overview** tab shows the node's OS, kernel,
architecture, CPUs, RAM, disk and agent version, plus its
[tags](/docs/guides/tags/). These are the node's *facts*:

- Forge asks the agent for them (`collect_facts`) when the node connects and
  then once a day. A failed attempt is retried after 10 minutes.
- Facts are stored, so they stay visible while the node is offline. The panel
  shows when they were collected.
- CPU, RAM and disk totals come from the latest live sample when there is
  one. A value taken from live telemetry says so in its tooltip.
- SSH host keys and other large fields are not stored.

To refresh facts now, open **Details** and select **Collect facts**. The node lists show the same specs, and
`GET /api/v1/servers` returns them in each node's `facts` field.

## Watch live metrics

The agent attaches a resource sample to every heartbeat: CPU, memory (the
cgroup limit when the agent runs in a container), swap, disk, network
throughput, load and uptime. Heartbeats are long polls, so a node that nobody
is looking at reports about every 25 seconds. While the Overview is open, the
control plane answers that node's heartbeats sooner, so the charts update at
the refresh interval you pick (3, 5, 10 or 30 seconds, or paused).

The live view is kept **in memory** on the control plane (up to 3,600 samples
per node, about an hour or two of live viewing) and is lost when Forge
restarts. Choose 5 minutes, 15 minutes or 1 hour above the charts.

CPU figures inside containers are the host's (Linux exposes no per-container
`/proc/stat`); memory honours the container's limit.

## Review a node's history

When an analytics database is set up (see
[Choose and connect databases](/docs/guides/databases/)), Forge records one row per
node and minute, plus the ten processes that used the most CPU and the ten
that used the most memory.

1. Open the node and select **Overview**.
2. Select **History** above the tiles.
3. Pick a **Window**: **1 hour**, **24 hours**, **7 days** or **30 days**.
   Windows up to 48 hours show one point per minute; longer windows show one
   point per hour.

The page shows **Peak CPU**, **Peak memory** and **Peak disk** with when they
happened, CPU, memory, disk and network trends, and **Top processes by CPU**
and **Top processes by memory** for the window. It refreshes every minute.

From the CLI:

```sh
forge nodes history <node> --hours 24
forge nodes report <node> --hours 168
```

Per-minute rows are kept 7 days and hourly trends 365 days by default
(`[analytics.retention]`). Platform administrators change this at runtime in
**Administration → Settings → Analytics**; see
[Configure analytics retention and limits](/docs/guides/analytics-settings/).

## Check storage

The **Storage** panel on the **Overview** tab lists every mounted local volume the agent sees: mount
point, device, file system, total, used, available and used %, with a total
row. Pseudo file systems (`tmpfs`, `proc`, …) and bind-mounted files are left
out. Volumes above 75 % turn amber, above 90 % red.

## Check connectivity between nodes

In the **Inter-node connectivity** panel on the **Overview** tab,
**Run checks** asks the node's agent to probe its peers: the other members of
its cluster and the control-plane node (the control-plane node itself checks
every node). Checks run *on the node*, so they show that node's own network
path. Each peer gets ICMP ping; when `ping` is missing, not permitted or
filtered, the agent measures a TCP connect instead: to the peer's SSH endpoint,
or to the control plane's agent port. Results show reachability, average
latency, packet loss, the method used and a latency trend over recent runs.
**Repeat** re-runs the checks every 30 s, 1 min or 5 min while the page is
open. The agent must allow the `probe` command (the installer adds it to
the allowlist).

## Find nodes in lists

The **Nodes** list shows **Active** nodes by default; switch to Disconnected,
Inactive (pending or revoked) or All. Search, cluster and role filters sit in
one row, with **Filter by tag** to keep only nodes that carry every selected
tag (see [Tag nodes](/docs/guides/tags/)). Each row shows the
node's specs. CPU and memory bars come from the latest sample. A cluster lists its
workers *and* the control-plane node, which takes part in every cluster.

## Inspect processes

The **Processes** tab, next to **Overview**, is an htop-like view of the
node: per-CPU, memory and swap meters, and the top processes with PID, user,
state, CPU%, MEM%, VIRT, RES, TIME+ and command.

It needs `server.manage` on the node because command lines can carry
sensitive arguments.

| Control | What it does |
| --- | --- |
| Column header | Sorts by that column. PID, CPU%, MEM%, RES, TIME+ and Command sort |
| **Refresh interval** | **Live (2s)**, **5s** (default), **10s** or **30s**. This browser remembers your choice |
| **Filter by name, command or user** | Case-insensitive substring match, run on the node |
| Refresh button | Fetches a snapshot now |

The panel polls only while it is open and the browser tab is visible. Each
refresh sends a signed, read-only command to the agent. Refreshes are not
audited; denials are.

What the panel shows and offers depends on the node's OS:

| | Linux | Windows |
| --- | --- | --- |
| User | The owner's name from `/etc/passwd`, else the uid | `NT AUTHORITY\SYSTEM`, `NT AUTHORITY\LOCAL SERVICE`, `NT AUTHORITY\NETWORK SERVICE`, `Window Manager\DWM-<n>`, `Font Driver Host\UMFD-<n>` or a local account name. Other owners, such as domain accounts, show as the SID (`S-1-5-21-…`) |
| State (S column) | The htop letter: `R`, `S`, `D`, `Z`, `T`, `X` or `?` | Hidden. Windows has no such states |
| **Load average** | Shown in the header | Hidden. Windows has no load average; the header shows CPU instead |
| Priority | The nice value of each process | Not shown. Forge does not read priority classes on Windows |
| Actions | **Send SIGTERM**, **Send SIGKILL**, **Send SIGHUP**, **Send SIGINT** (each optionally with child processes), **Renice…** | **End task**, **End process**, **End process tree**, **Priority class…** |
| How actions run | The node's root helper runs them, derived only from the signed command | The agent (LocalSystem) runs `taskkill.exe` and the built-in Windows PowerShell by absolute path, with fixed arguments |

## End a process or change its priority

Use the row menu in the **Processes** panel. The menu appears only when you
hold `process.manage` on the node. The CLI equivalents are
[`forge nodes kill`](/docs/reference/cli/nodes/#forge-nodes-kill) and
[`forge nodes priority`](/docs/reference/cli/nodes/#forge-nodes-priority).

1. Open the node, then the **Details** tab, and find the process in the
   **Processes** panel. Type part of its name in
   **Filter by name, command or user** to narrow the list.
2. Open the row's menu and pick the action:

   | Goal | Linux | Windows |
   | --- | --- | --- |
   | Ask the process to exit | **Send SIGTERM** | **End task** (asks the process to close) |
   | Force it to end | **Send SIGKILL** | **End process** (forced, like `taskkill /F`) |
   | End it with its descendants | Any **Send …** action with **Include child processes** | **End process tree** |
   | Reload a daemon's configuration | **Send SIGHUP** | Not available |
   | Interrupt it, like Ctrl+C | **Send SIGINT** | Not available |
   | Change its priority | **Renice…**: a nice value from `-20` (highest priority) to `19` | **Priority class…**: **Idle**, **Below normal**, **Normal**, **Above normal** or **High** |

3. Read the confirmation dialog, which names the process and the node, and
   confirm.
4. If Forge shows **Verify it's you**, confirm with your passkey or a 6-digit
   code. Forge then runs the action.

:::caution
**Send SIGKILL** and **End process** give the process no chance to save its
state. Try **Send SIGTERM** or **End task** first. Forge never sets the
Windows `realtime` class, because it can starve the system's own threads.
:::

The node acts only when the PID still belongs to the process you saw in the
list. It refuses Orbyn Forge's own processes, init and kernel threads on
Linux, and the core Windows system processes such as `lsass` and `csrss`.
For a tree, it checks every process before it acts on any. The full list is
in the [REST API reference](/docs/reference/rest-api/#processes).

Each action is audited as `server.process.signal` or
`server.process.priority`, with the PIDs it affected. A refusal by the node
is audited as a failure. See [Audit](/docs/concepts/audit/).

## See the services on a node

The **Services** tab, after **Processes**, lists every service and
service placed on the node: its name (a link to the service page),
environment and cluster, kind (`native`, `container` or `job`), role (`active`
or `standby`), the last deployment result under **Deploy**, and the live
**State**, and its **Health** (**Healthy**, **Degraded**, **Unhealthy** or
**Unknown**; select it for each check). The panel description counts how many
are running, failed or stopped. Health is loaded as rows come into view;
compose stack services show a dash. See
[Health checks](/docs/concepts/health-checks/#status-per-node).

| **State** | Meaning |
| --- | --- |
| `running` | The node last reported the service or container as running |
| `stopped` | The node reported it stopped (inactive or exited) |
| `failed` | The service failed, or its last deployment failed. Hover for the error |
| `node offline` | The node's agent is offline, so Forge cannot tell. Hover for the last reported state |
| `deploying`, `starting`, … | A job is running on that node |
| `not deployed` | Placed on the node but not deployed yet |
| `unknown` | The node has not reported a state yet |

**State** is what the node last reported, plus the agent's connection. It is
not a health check. The tab updates on its own when the node goes offline or
comes back, and when a job finishes.

The row menu has **Logs**, **Start**, **Restart** and **Stop**, the same
per-node actions as the service page. **Logs** opens a log tab in the
bottom panel that follows the service on this node. **Restart** and **Stop** ask you to
confirm first. The actions are disabled, with the reason, when you lack the
`deploy` permission (roles **Developer**, **Operator** or higher), when the
node is offline, when the service is not deployed yet, or when it belongs to
a compose stack (act on the stack instead).

## Watch container resources and collection

The node's **Overview** tab shows **Containers** charts below the node
charts: CPU, Memory and Network (in + out), one line per container for the
8 busiest ("Showing the N busiest of M containers"). Each container's own
history is on its page (see [Containers](/docs/guides/containers/)).

To change what the node collects:

1. Open the node's **Details** tab and find **Container telemetry**. You
   need `server.manage` on the node (**Operator** or higher).
2. Turn **Collect container logs and metrics** off to stop collection, or
   type a container name or a `key=value` label under **Excluded
   containers** and select **Exclude** (at most 64 entries).
3. Select **Save** in the unsaved-changes bar. The node picks it up within
   one heartbeat. The change is audited as
   `server.container_telemetry.update`.

When analytics is off, the charts and stored logs say so and link to
**Settings**. See [Container log collection](/docs/concepts/container-logs/).

## Rename a node

Select **Rename node** (the pencil next to the node's name). Only the console
name changes; the machine hostname, SSH endpoint and agent identity stay the
same. To cut a node off, see [Revoke a node](/docs/guides/add-nodes/#revoke-a-node).

## Verify

- The **System** panel shows when the facts were collected, and the time
  moves forward after **Collect facts**.
- The charts on **Overview** draw new points at the refresh interval you
  picked.
- **Run checks** fills the connectivity table with a result per peer.
- After you end a process, it is gone from the **Processes** panel on the
  next refresh. After you change its priority, the nice value in its row
  changes on Linux. On Windows, check the class in Task Manager
  (**Details** tab, **Set priority**) on the node.
- The audit log has a `server.process.signal` or `server.process.priority`
  entry with the affected PIDs.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| **Services** shows `node offline` for every row | The node's agent is offline | Check the node's status; the rows update when it reconnects |
| Charts are empty or stop | The node is offline, or the control plane restarted and lost the in-memory history | Check the node's status; history refills while you watch |
| **Run checks** is disabled | The agent is offline, or the node has no peers (**No peers to check**) | Bring the node online; add nodes to its cluster |
| Connectivity or **Processes** answers "the node refused" | The agent's `allowed_commands` lacks the command (`probe` or `processes`), or the agent is older than the control plane | [Update the agent](/docs/guides/update-agents/), or add the command to `allowed_commands` |
| The row menu is missing in **Processes** | You lack `process.manage` on the node | Ask an admin for the **Operator** role or higher on the node |
| **Verify it's you** appears, or the CLI exits with `5` (`step_up_required`) | Process actions need an MFA-verified session | Confirm with a passkey or a code. In scripts, run `forge auth step-up` first. See [Multi-factor authentication](/docs/guides/mfa/) |
| "process `<pid>` is no longer the one listed: it exited and the PID was reused (refresh the list)" | The process ended after the list was read, and another process now has its PID | Refresh the list and pick the process again |
| "refused: … is part of Orbyn Forge", "is a Windows system process", "is the init system" or "is a kernel thread"; or "process `<pid>` in the tree is protected" | The process, or one of its descendants, is protected | Forge does not act on it. Manage Forge's own components through their service or the node installer |
| **End task** fails with "This process can only be terminated forcefully" | The agent runs as a Windows service in session 0, so it cannot ask desktop apps and console programs to close | Use **End process**, or `forge nodes kill <node> <pid> --signal kill` |
| "the node refused: command `process_action` is not allowed by this agent's local policy" | `process_action` was removed from `allowed_commands` in the agent's `agent.toml` | Add `"process_action"` to `allowed_commands` and restart the agent. See [Configuration](/docs/reference/configuration/#agent-agenttoml) |
| An action answers "the node refused" with an unknown-command error | The agent is older than the control plane | [Update the agent](/docs/guides/update-agents/) |
| "the node did not answer in time" | The node did not answer within 30 seconds | Check that the node is online, then try again |
| **History is not available**: "analytics is off on this server" | No analytics engine is set | Set `[analytics] engine`; see [Choose and connect databases](/docs/guides/databases/) |
| **History is not available**: "analytics is unavailable" | The analytics database is starting or cannot be reached | Run `forge nodes analytics` and read `last_error` |
| History stops at some time while the node stays online | The node cannot reach the analytics worker on TCP `55132`; its history waits in the node's spool | Open TCP `55132` from the node to the control plane. The spool uploads the backlog, up to its cap (512 MiB, 72 hours by default) |
| History has no top processes | The agent is older than the control plane | [Update the agent](/docs/guides/update-agents/) |
| Facts look stale | Facts refresh once a day | Select **Collect facts** on **Details** |
| CPU looks too high for a container | Inside containers, CPU figures are the host's | Read memory, which honours the container's limit |

## Next steps

- [Tag nodes](/docs/guides/tags/)
- [Update agents](/docs/guides/update-agents/)
- [Browser terminal (SSH)](/docs/guides/terminal/)
- [`forge nodes kill`](/docs/reference/cli/nodes/#forge-nodes-kill) and
  [`forge nodes priority`](/docs/reference/cli/nodes/#forge-nodes-priority)
- [Processes in the REST API](/docs/reference/rest-api/#processes)
- [Roadmap](/docs/roadmap/) for long-term metrics
