---
title: Create an alert rule
description: Get an alarm when a node goes offline, acknowledge it, and see it resolve.
sidebar:
  order: 22
---

This guide sets up a rule that opens an alarm when a node has been offline
for a minute, and shows how to work with the alarm. For how rules and
alarms behave, see [Alerts](/docs/concepts/alerts/). A rule only opens alarms; to
be told about them, see
[Send alarm notifications](/docs/guides/alarm-notifications/).

## Before you begin

- You need `alert.manage` on the workspace (**Operator**, **Workspace
  admin** or **Platform admin**) to create rules and acknowledge alarms.
  `alert.view` is enough to see them.
- At least one enrolled node. Linux and Windows nodes are both supported.

## Create the rule

1. Open **Operations → Alerts** and select **Rules**.
2. Select **New rule**.
3. Leave **From template** at **Blank rule** and fill in the form. (The
   **Node offline** template fills in the same rule with a two-minute
   duration; see [Create alerts from templates](/docs/guides/alert-templates/).)
   - **Name**: `Node offline`
   - **Severity**: `critical`
   - **Subject**: `node.agent.status`
   - **Watches**: **Nodes**; leave all off to watch every node
   - **Condition**: **State is**, **States** `offline`
   - **When**: **For a duration**, **Minutes** `1`
   - **Recovers when**: `online`, **Hold (minutes)** `0`
4. Select **Save**.

With the CLI, save the rule as `node-offline.json`:

```json
{
  "name": "Node offline",
  "severity": "critical",
  "scope": { "kind": "servers" },
  "subject": "node.agent.status",
  "spec": {
    "trigger": { "kind": "state_in", "states": ["offline"] },
    "mode": { "mode": "sustained", "for_ms": 60000 },
    "recovery": { "condition": { "kind": "state_in", "states": ["online"] }, "hold_ms": 0 }
  }
}
```

and create it:

```sh
forge alerts create-rule @node-offline.json
```

## Alert on high CPU

The same form watches numbers. For "CPU above 85 % for 30 minutes, clear
below 75 % for 5 minutes":

- **Subject**: `node.cpu.percent` (the condition switches to **Number**)
- **Compare** `>`, **Threshold** `85`
- **When**: **For a duration**, **Minutes** `30`
- **Recovers past**: `75`, **Hold (minutes)**: `5`

To react to the average instead of every sample, pick **Average over a
window**: one short spike does not fire it. **Any spike in a window**
fires on a single high sample.

## Work with an alarm

1. When a node has been offline for a minute, the alarm appears under
   **Alarms** with the state **firing**.
2. Select the alarm, optionally type a note, and select **Acknowledge**. The
   alarm keeps firing; the timeline shows who acknowledged it.
3. When the node is back online, the alarm turns **resolved**.

With the CLI:

```sh
forge alerts alarms --active
forge alerts ack <alarm-id> --note "Rebooting the node"
```

## Verify

- `forge alerts rules` lists the rule with revision `1`.
- Stop the agent on a test node (`systemctl stop forge-agent`). After about
  `offline_after_secs` plus one minute, `forge alerts alarms --active` shows
  the alarm. Start the agent again and the alarm resolves within 15 seconds.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `400` "the subject is lowercase dotted segments…" | The subject has capitals, spaces or empty parts | Use a subject such as `node.agent.status` |
| `400` "invalid rule condition" | The `spec` JSON does not match the condition format | Compare it with the example above or the [REST reference](/docs/reference/rest-api/#alerts) |
| The alarm turned **ended** instead of **resolved** | The rule was changed, disabled or deleted, or the node was removed, while it fired | Expected: Forge ends tracking without claiming recovery |
| No alarm although the node is offline | The node is not in the rule's **Nodes**, the rule is disabled, or the duration has not passed | Check the rule; evaluation runs every 15 seconds |

## Next steps

- [Send alarm notifications](/docs/guides/alarm-notifications/): send alarms to
  a webhook or a Slack-compatible chat.
- [Alerts](/docs/concepts/alerts/): time conditions, unknown data and recovery.
- [`forge alerts`](/docs/reference/cli/alerts/): every command and flag.
