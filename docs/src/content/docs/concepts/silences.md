---
title: Silences and maintenance windows
description: How a silence holds back alarm notifications for a bounded time without changing evaluation, and what is sent when it ends.
sidebar:
  order: 7.5
---

A silence holds back the notifications of the alarms it covers for a
bounded time. This page explains what a silence covers, what it never
changes, and what your channels receive when it ends. For the steps, see
[Silence alarms and schedule maintenance](/docs/guides/silence-alarms/).

## One object, two uses

A **silence** starts now. You create it when an alarm is already firing and
you do not want more messages about it, for example while you work on the
problem.

A **maintenance window** is a silence whose start lies in the future. You
create it before planned work, such as a kernel upgrade or a network
change. It is `scheduled` until its start, `active` from its start, and
`expired` from its end. Forge has one object and one mechanism for both.

```diagram
alt: Silence states
states:
  path:
    - { state: created, edge: "" }
    - { state: scheduled, edge: starts_at }
    - { state: active, edge: ends_at, tone: accent }
    - { state: expired, tone: muted }
  transitions:
    - { from: scheduled, to: expired, when: end }
    - { from: active, to: expired, when: end }
```

The end is exclusive: at `ends_at` the silence no longer holds. A silence
lasts from one minute to 30 days, and can start up to 365 days ahead. A
workspace has at most 200 silences that have not ended, and one person at most 50. People with `alert.manage` on the whole workspace are exempt from the workspace cap, so narrow roles cannot use it up during an incident.

## Evaluation never changes

A silence is checked in one place: just before Forge sends a message. The
evaluator never reads it. While a silence is active:

- Alarms still open, fire, resolve and end as if no silence existed.
- The alarm list and each alarm's timeline stay complete.
- Acknowledgements and notes work as usual.
- A firing alarm that a silence covers shows the silence (the console marks
  it **silenced until …**), so you can tell why nobody was paged.

Silencing and acknowledging are independent. Acknowledging an alarm stops
its reminders; it does not silence it. Silencing an alarm does not
acknowledge it.

## What a silence covers

A silence matches alarms by where their resource is, and optionally by rule
and severity. Its **scope** is one of:

| Scope | Covers |
| --- | --- |
| Workspace | Every alarm of the workspace |
| Cluster | Alarms on the cluster's nodes and on resources under the cluster. Service alarms in an environment that is not under the cluster are not covered |
| Environment | Alarms on the environment's services and nodes |
| Nodes | Alarms on these nodes and on the packages (databases included) installed on them. Alarms of services that run on them are not covered: silence the environment or the services as well |
| Services | Alarms on these services |

Two optional filters narrow it:

- **Rules**: only alarms of these alert rules. Empty means every rule.
- **Severity**: a lower bound (`min_severity`, this severity and worse) and
  an upper bound (`max_severity`, this severity and milder).

:::caution
An alarm worse than `max_severity` is not silenced and still notifies. Use
the upper bound during maintenance so that a `critical` alarm still pages
someone while expected `warning` alarms stay quiet.
:::

A silence matches the alarm, not an alarm id. If an alarm resolves and the
same problem fires again inside the window, the new alarm is covered too.
When several active silences cover one alarm, Forge records the one that
ends last.

What a silence matches is fixed when you create it. You can only extend it
or end it early. This keeps the audit record of its creation true for its
whole life.

## What happens to messages

Forge still plans a delivery for every alarm change, as it does without a
silence. When a delivery is due and an active silence covers its alarm,
Forge records it as `suppressed` with the silence's id and reason, and does
not send it. The delivery history therefore shows every message that a
silence held back.

When the silence ends (at its end time, or because someone ended it),
Forge settles what it held back, alarm by alarm and channel by channel.
Each channel gets at most one message:

| Alarm when the silence ends | Message per channel |
| --- | --- |
| Still firing, and its `fired` message was held back | One `fired` message |
| Still firing, and only reminders were held back | One `reminder` |
| Resolved or ended, after the channel had been told it fired | One `resolved` or `ended` message |
| Fired and resolved inside the silence | Nothing |

Each of these messages is marked as following a silence: the webhook
payload carries `after_silence.silence_id`, and the Slack-compatible text
adds the line `Sent now because a silence that held this back has ended.`
See the [notification payload reference](/docs/reference/notification-payload/#after-a-silence).

If another active silence still covers the alarm when the first one ends,
the held-back deliveries move under that silence and nothing is sent yet.

Reminders are suppressed like any other message. Reminders that come due
during a silence are not replayed afterwards; at most one message stands
for all of them.

A recovery goes only where the alarm was announced. If the `fired` message
to a channel was suppressed and the alarm then recovers while still
silenced, the recovery is cancelled with the reason
`the firing notification was silenced, so there is nothing to recover from`.

## Who may silence what

| Task | Action | Roles |
| --- | --- | --- |
| Create, extend or end a silence | `alert.manage` on **everything the scope names** | **Operator**, **Workspace admin**, **Platform admin** |
| Read a silence | `alert.view` on the workspace, or on everything its scope names | Every role except **Customer** |

`alert.manage` and `alert.view` apply to the workspace, a cluster, a
environment, a node and a service. A person with the **Operator** role
on one environment can silence that environment and nothing wider. A
workspace-wide silence needs `alert.manage` on the workspace.

A silence that you may not read answers `404`, the same as one that does
not exist. Every creation, extension and end is audited as
`alert.silence.create`, `alert.silence.extend` or `alert.silence.end`, with
the scope, rules, severities, window and reason. A refusal is audited too.

## Retention

A silence that has ended stays visible as history. Forge deletes it 30 days
after its end, once no kept delivery names it.

## Limitations

- A silence cannot be edited apart from its end. To change what it covers,
  end it and create another one.
- A silence cannot repeat. Create one silence per maintenance window.
- A silence never changes alarm evaluation. To stop a rule from opening
  alarms, turn the rule off.
- Test messages name no alarm, so no silence covers them.

## Related

- [Alerts](/docs/concepts/alerts/)
- [Silence alarms and schedule maintenance](/docs/guides/silence-alarms/)
- [REST API: silences](/docs/reference/rest-api/#silences)
- [`forge alerts silences`](/docs/reference/cli/alerts/#silences)
