---
title: Silence alarms and schedule maintenance
description: Hold back alarm notifications now or during planned work, then extend or end the silence.
sidebar:
  order: 22.7
---

This guide shows how to silence a firing alarm, schedule a maintenance
window before planned work, and extend or end a silence. Alarms are still
evaluated and recorded while a silence holds; only their notifications
wait. For how silences behave, see
[Silences and maintenance windows](/docs/concepts/silences/).

## Before you begin

- You need `alert.manage` on everything the silence's scope names: the
  workspace, a cluster, an environment (environment), or each node or
  service. The **Operator**, **Workspace admin** and **Platform admin**
  roles hold it. An **Operator** of one environment can silence only that
  environment.
- `alert.view` is enough to see silences.
- Alarm notifications are set up. See
  [Send alarm notifications](/docs/guides/alarm-notifications/).
- Linux and Windows nodes are covered alike: a silence acts on the server,
  not on the node.

## Silence a firing alarm

1. Go to **Alerts**. On the **Alarms** tab, find the firing alarm.
2. Select the **Silence** button on its row. The **New silence** dialog
   opens with the alarm's node or service and its rule filled in.
3. Under **Starts**, keep **Now**, and choose how long the silence lasts,
   from one minute to 30 days.
4. Type a **Reason**, for example `Investigating disk usage on web-01`. The
   reason is required and shows on the alarm and in the delivery history.
5. Select **Silence**. The alarm now shows **silenced until …**.

The silence covers the alarm's resource, so a new alarm on the same node or
service inside the window is covered too.

With the CLI or the API, name the resource in the scope:

```sh
forge alerts silences create --node <node-name> --duration 2h \
  --reason "Investigating disk usage on web-01"
```

```sh
curl -sS -X POST "$FORGE_URL/api/v1/silences" \
  -H "Authorization: Bearer $FORGE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"scope": {"kind": "servers", "ids": ["<server-id>"]},
       "duration_secs": 7200,
       "reason": "Investigating disk usage on web-01"}'
```

## Schedule a maintenance window

1. Go to **Alerts** and open the **Silences** tab.
2. Select **New silence**.
3. Choose the **Scope**: the workspace, a cluster, an environment, nodes or
   services.
4. Optionally limit it to some rules, or set the highest severity it
   silences. Set the highest severity to `warning` (or `major`) so that a
   `critical` alarm still pages someone during the work.
5. Under **Starts**, select **Later** and pick the start, then set the end
   or a duration. The window can start up to 365 days ahead and last up to
   30 days.
6. Type a **Reason**, for example `Kernel upgrade, change <change-id>`, and
   select **Plan window**.

The silence shows as `scheduled` until its start and `active` from then on.

```sh
forge alerts silences create --cluster <cluster-id> \
  --starts-at 2026-12-01T22:00:00Z --ends-at 2026-12-02T02:00:00Z \
  --max-severity major --reason "Kernel upgrade, change <change-id>"
```

Through the API, `starts_at` and `ends_at` are epoch milliseconds:

```sh
curl -sS -X POST "$FORGE_URL/api/v1/silences" \
  -H "Authorization: Bearer $FORGE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"scope": {"kind": "cluster", "id": "<cluster-id>"},
       "max_severity": "major",
       "starts_at": 1764626400000,
       "ends_at": 1764640800000,
       "reason": "Kernel upgrade, change <change-id>"}'
```

:::caution
A workspace-wide silence holds back every alarm notification of the
workspace, from every node and service. Prefer the narrowest scope
that covers the work, and set a highest severity.
:::

## Extend a silence

Work takes longer than planned. Extend the silence before it ends; an
expired silence cannot be extended.

1. Go to **Alerts** > **Silences** and open the silence's row menu.
2. Select **Extend…**, choose **+1 hour**, **+4 hours**, **+1 day** or a
   new end, and select **Extend**. The silence can last at most 30 days
   from its start.

```sh
forge alerts silences extend <silence-id> --by 1h
```

```sh
curl -sS -X POST "$FORGE_URL/api/v1/silences/<silence-id>/extend" \
  -H "Authorization: Bearer $FORGE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"extend_secs": 3600}'
```

## End a silence

End a silence when the work is done, or cancel a maintenance window that
has not started.

1. Go to **Alerts** > **Silences** and open the silence's row menu.
2. Select **End now…** (for a scheduled window, **Cancel window…**) and
   confirm.

```sh
forge alerts silences end <silence-id> --yes
```

Alarms that the silence covered and that still fire notify within seconds,
with one message per channel. An alarm that fired and resolved during the
silence sends nothing. The ended silence stays in the list as history.

## Verify

- `forge alerts silences list --state active` lists the silence.
- The firing alarms it covers show **silenced until …** in **Alerts**, and
  `silence` in `GET /api/v1/alarms`.
- The delivery history (`forge alerts deliveries list --alarm <alarm-id>`,
  or `GET /api/v1/notification-deliveries?status=suppressed`) shows the
  held-back messages as `suppressed`.
- After the silence ends, a channel that still has a firing alarm receives
  one message with `after_silence` in the payload.
- The audit log has `alert.silence.create`, `alert.silence.extend` and
  `alert.silence.end` entries.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `403` when you create a silence | You lack `alert.manage` on something the scope names, or an id in the scope does not exist | Narrow the scope to what you manage, or ask a **Workspace admin** |
| `422` "the start is in the past" | `starts_at` is more than 5 minutes in the past | Leave `starts_at` out to start now |
| `422` "a silence lasts at most 30 days from its start" | The window or the extension is longer than 30 days | Create another silence after this one |
| `422` "not later than the current end; to stop a silence, end it" | The new end of an extension is not later than the current end | Use **End** to stop early |
| `409` "the silence has ended; create a new one" | You tried to extend or end an expired silence | Create a new silence |
| `409` "a workspace has at most 200 silences that have not ended" | Too many active or scheduled silences | End silences you no longer need, or ask a workspace administrator |
| `409` "one person has at most 50 silences that have not ended" | You have too many active or scheduled silences | End your silences you no longer need |
| A `critical` alarm still notified during the silence | Its severity is above the silence's highest severity | Expected. Raise `max_severity` only if a `critical` alarm must stay quiet |
| No message after the silence ended | The alarm fired and resolved inside the silence, or another active silence still covers it | Expected. Check **Alerts** > **Silences** for an overlapping silence |

## Next steps

- [Silences and maintenance windows](/docs/concepts/silences/): what a silence
  covers and what is sent when it ends.
- [Notification payload](/docs/reference/notification-payload/#after-a-silence):
  the `after_silence` field.
- [REST API: silences](/docs/reference/rest-api/#silences)
- [`forge alerts silences`](/docs/reference/cli/alerts/#silences)
