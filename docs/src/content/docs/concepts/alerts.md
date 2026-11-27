---
title: Alerts
description: How alert rules turn evidence about nodes and services into alarms, how missing data is handled, and how channels, policies and deliveries notify people.
sidebar:
  order: 7
---

This page explains how Forge decides that something is wrong, how long an
alarm lasts, why missing data never counts as good news, and how alarms
reach people through notification channels.

## Rules, series and alarms

An **alert rule** watches one **subject** (a kind of evidence, such as
`node.agent.status`) on the resources in its **scope**: nodes, services,
databases and other package installations, or ingress certificates; all of
one kind in the workspace, or the ones you pick. Each resource the rule
watches (each instance, for a service) is one **series**.

When a series meets the rule's condition, Forge opens an **alarm**: one
problem episode with a start, a timeline and an end. A series has at most
one firing alarm at a time. If the problem comes back after it recovered,
Forge opens a new alarm and links it to the previous one.

```diagram
alt: Alarm states
states:
  path:
    - { state: Normal, edge: condition holds }
    - { state: Pending, edge: long enough, tone: warn }
    - { state: Firing, edge: recovery holds, tone: danger }
    - { state: Resolved, tone: ok }
  transitions:
    - { from: Pending, to: Normal, when: condition stops }
    - { from: Firing, to: Ended, when: "rule changed or deleted, node removed", tone: muted }
```

- **Pending** only exists for conditions with a duration ("for 30 minutes").
- **Resolved** means valid evidence showed recovery for the hold time.
- **Ended** means Forge stopped tracking without recovery evidence: the rule
  was deleted, disabled or changed, or the node was removed. Forge never
  pretends such an alarm recovered.

**Acknowledged** is not a state. It records that someone is working on the
alarm; the alarm keeps firing until it recovers. Notes go on the timeline.

## Time conditions

| Mode | Example | Meaning |
| --- | --- | --- |
| Immediate | node is `offline` | The latest valid evidence meets the condition |
| Sustained | CPU > 85 % for 30 min | Every valid sample for 30 minutes met it, without a gap in the evidence |
| Window average | average CPU over 30 min > 80 % | The time-weighted average, with at least a minimum share of the window covered by evidence. One spike does not dominate |
| Repeated | > 85 % at least 5 times in 30 min | Distinct breaches, each starting after the condition was false. Breaches closer than the minimum spacing count once, so faster sampling cannot inflate the count |
| Any in window | maximum CPU in 30 min > 85 % | A single spike qualifies. It clears once the spike has left the window |

**Recovery** has its own condition and a hold time, so an alarm does not
flap around one threshold. For example, CPU > 85 % for 30 minutes fires,
and CPU < 75 % for 5 minutes clears it.

## Missing data is unknown

Each piece of evidence counts for a limited time. After that, Forge treats
the series as **unknown**, not as healthy and not as failing:

- Unknown interrupts a pending condition: the 30 minutes start again when
  evidence returns.
- Unknown never resolves a firing alarm. The alarm stays firing until
  valid recovery evidence holds for the hold time.
- Late or duplicate evidence is ignored. Evidence with a timestamp in the
  future is refused.

Forge keeps each series' timers in the database, so restarting Forge does
not reset a 25-minute-old sustained condition.

## Where the evidence comes from

Evidence reaches the evaluator in three ways:

- **Heartbeat telemetry.** Each agent heartbeat carries the node's
  measurements. The server records them as observations about the node, at
  most every 10 seconds per node. They count for three heartbeat intervals
  (75 seconds by default).
- **Health probes.** Health checks of services report states per
  instance. See [Health checks](/docs/concepts/health-checks/).
- **The control plane.** Some states Forge already holds in its own
  records: agent status, deployments, package installations and
  certificates. The evaluator reads them on every pass (every 15 seconds),
  and each value counts for two passes.

A node that stops reporting becomes unknown, so a CPU or disk rule neither
fires nor resolves for it. Use a `node.agent.status` rule to catch the node
itself.

## Subjects

Every subject a rule can watch. `GET /api/v1/alert-subjects` and
`forge alerts subjects` list the same table, plus the health check names
seen in your workspace. Forge refuses a rule whose subject is not in this
table, or whose scope is of another kind, so a typo cannot create a rule
that never fires.

| Subject | Values | Scope | Source |
| --- | --- | --- | --- |
| `node.agent.status` | `online`, `offline`, `pending` | nodes | Control plane: `offline` when the agent has not been heard from for `offline_after_secs`; `pending` when the node enrolled but never connected |
| `node.cpu.percent` | number, 0–100 | nodes | Heartbeat: CPU use across all cores |
| `node.mem.used_percent` | number, 0–100 | nodes | Heartbeat: memory in use |
| `node.disk.used_percent` | number, 0–100 | nodes | Heartbeat: the root filesystem (`/` on Linux, `C:\` on Windows) |
| `node.disk.max_used_percent` | number, 0–100 | nodes | Heartbeat: the fullest local volume. Removable media and virtual filesystems (`tmpfs`, `devtmpfs`, `overlay`, `squashfs`, `iso9660` and kernel views) do not count. The observation detail names the mount point |
| `node.swap.used_percent` | number, 0–100 | nodes | Heartbeat: swap in use. A node without swap reports `0` |
| `node.load.per_cpu` | number (ratio) | nodes | Heartbeat: one-minute load average divided by the CPU count. Unknown on Windows and on agents that do not report a CPU count |
| `node.uptime.seconds` | number, seconds | nodes | Heartbeat: time since the node booted. A drop means it rebooted |
| `node.cert.expires_in_days` | number, days | nodes | Control plane: days until the agent's client certificate expires. The agent renews it before then; a low value means renewal is failing |
| `app.instance.state` | `active`, `inactive`, `failed` | services | Control plane: the state each node last reported for its instance of the service. One series per node |
| `app.deploy.status` | `pending`, `deploying`, `deployed`, `failed`, `drifted`, `stranded` | services | Control plane: the deployment status of each instance. One series per node |
| `package.state` | `running`, `stopped`, `not-installed` | databases and packages | Control plane: the service state the package recipe last reported, per installation |
| `package.status` | `pending`, `installing`, `installed`, `updating`, `starting`, `stopping`, `restarting`, `removing`, `checking`, `failed` | databases and packages | Control plane: the lifecycle status of each installation. `failed` after a failed install, upgrade or other package job |
| `tls.cert.expires_in_days` | number, days (negative once expired) | certificates | Control plane: days until an uploaded ingress certificate expires. Let's Encrypt certificates renew themselves and are not covered |
| `health.readiness.<check>` | `pending`, `passing`, `failing`, `disabled` | services | Health probes, per instance |
| `health.liveness.<check>` | `pending`, `passing`, `failing`, `disabled` | services | Health probes, per instance |
| `health.startup.<check>` | `waiting`, `succeeded`, `exhausted` | services | Health probes, per instance |

An unknown value (`unknown`, a missing report, a stale measurement) is never
one of these values: it is no evidence.

A newer agent may report a `node.*` measurement this server does not know
yet. Forge accepts such a subject on a node rule unless it is a near miss of
a known one, in which case it suggests the right name.

:::note
A database port check (`package.port.reachable`) is not available. Health
probes run per service instance, not per package installation, so
there is no probe that could test a database's own port yet. Use
`package.state` and the database host's disk and memory instead.
:::

### Scopes

A rule's scope picks the kind of resource it watches, and optionally which
ones. No ids means every resource of that kind in the workspace, including
ones added later.

| Scope kind | Resources | An alarm is visible to |
| --- | --- | --- |
| `servers` | Nodes | Whoever may `view` the node |
| `services` | Services, one series per instance | Whoever may `view` the service |
| `installations` | Package installations, databases included | Whoever may `view` the installation's node |
| `certificates` | Uploaded ingress certificates | Whoever may `view` the workspace |

Rules can start from [templates](/docs/reference/alert-rule-templates/): ready-made
rules for nodes, services, databases and certificates. See
[Create alerts from templates](/docs/guides/alert-templates/).

## Who sees what

- **`alert.view`** on the workspace (every role except **Customer**) lists
  rules and alarms. An alarm on a node you cannot `view` stays hidden.
- **`alert.manage`** (**Operator**, **Workspace admin**, **Platform admin**)
  creates, changes and deletes rules and notification policies, acknowledges
  alarms and adds notes. These actions are audited.
- **`notification.manage`** (**Workspace admin**, **Platform admin**, with an
  MFA-verified session) creates, changes, deletes and tests notification
  channels. See [Why channels are stricter than policies](#why-channels-are-stricter-than-policies).

## Notifications

Rules decide that something is wrong. Notifications tell people about it.
Three objects do that:

- A **channel** is where a message goes. A `webhook` channel receives
  Forge's JSON document by `POST`, optionally signed. A `slack` channel
  receives `{"text": "…"}`, the format of Slack incoming webhooks, which
  Mattermost and Discord's `/slack` endpoint also accept. An `email` channel
  sends a plain-text email through an SMTP server you name, to up to 20
  recipients.
- A **policy** says which alarms notify which channels. It matches alarms by
  minimum severity, optionally by rule and by resource, and listens for the
  events `fired`, `resolved` or both. A policy can also email **people**, by
  user and by role, through one email channel.
- A **delivery** is one message intent: one event of one alarm, for one
  channel of one policy. It records every attempt to send it.

```diagram
alt: Alarm notification delivery pipeline
flow:
  steps:
    - { label: alarm changes, note: "fired, resolved, ended", edge: same transaction }
    - { label: outbox, edge: plan }
    - { label: deliveries, note: "one per policy and channel", edge: dispatch }
    - { label: attempts, note: "retried with backoff", edge: "" }
    - { label: receiver, tone: accent }
```

Forge writes each alarm change and its outbox row in one database
transaction, so an alarm cannot open or close without leaving a row to
notify about. A background pass, every 5 seconds, turns each outbox row into
deliveries and marks the row planned in one transaction. It then sends the
deliveries that are due and records each attempt.

### How people are chosen

A policy's email users and roles are resolved for each alarm when its
delivery is planned, not when you save the policy:

- A **named user** is emailed only while they hold `alert.view` on the
  alarm's resource. Naming someone in a policy never shows them an alarm
  they could not open in the console.
- A **role** stands for the people responsible for the alarm's resource:
  everyone who holds that role on the resource itself or on any resource
  above it (its environment, cluster, workspace, or the platform). A role
  held on a sibling, such as another environment of the same project,
  does not count. Role holders pass the same `alert.view` check.
- Disabled accounts and accounts without an email address are skipped. Each
  address gets one message, however many ways it was reached.

The result is kept with the delivery. Retries and the message after a
silence go to the same addresses, even when someone's role or address
changes in between. The next alarm event is resolved again.

```diagram
alt: How a notification policy resolves recipients
flow:
  steps:
    - { label: "policy: users + roles", edge: plan }
    - { label: "named users with `alert.view`", note: "+ role holders on the resource or above it\n− disabled, − no address, deduplicated", edge: "" }
    - { label: one delivery, note: addresses kept with it, tone: accent }
```

One delivery goes to all of a policy's people through its email channel, as
blind copies: nobody sees the other addresses. The channel's own recipients
are not added; they get the channel's message only when the channel is also
one of the policy's channels. When nobody is left, the delivery is recorded
as `cancelled` with `no recipients`, so the history shows that the policy
matched.

Skipped people are recorded by user id and reason (`no_email`, `no_access`,
`disabled`), never by address. Only people with `notification.manage` see
the addresses a delivery went to; others see how many there were.

### Why evaluation and notification are separate

Sending never writes to an alarm, a rule or a job. A receiver that is slow
or down can delay a message or lose it. It cannot delay evaluation, undo an
alarm or open one twice. The alarm list stays the record of what happened,
and the delivery history is the record of who was told.

### Delivery guarantees

- **At least once.** If a receiver accepts a message and its answer is lost,
  Forge sends the message again. Every attempt of a delivery carries the
  same delivery id (the `X-Forge-Delivery` header and `delivery_id` in the
  payload), so a receiver can drop repeats.
- **No duplicates from planning.** A delivery has a unique key made of the
  alarm, the event, the reminder number, the policy and the channel.
  Planning the same alarm change again adds nothing.
- **One message per channel.** When two policies match the same alarm event
  and name the same channel, the channel gets one message.
- **Retries for temporary failures.** A `2xx` answer means sent. Forge
  retries timeouts, connection and TLS failures, and the statuses `408`,
  `425`, `429` and `5xx`, with a wait that doubles each time, up to the
  configured number of attempts. Then the delivery is `failed`.
- **No retries for refusals.** Any other status (`3xx`, other `4xx`), a
  blocked target, a deleted signing secret or an unusable stored URL fails
  the delivery at once. Sending again would not change the answer; a person
  has to fix the channel.
- **Stale messages are cancelled.** A firing message or a reminder that is
  still waiting when the alarm stops firing is not sent. Neither is a
  message for a channel that was turned off or deleted. The delivery shows
  `cancelled` with the reason.
- **Recovery only where the alarm was announced.** A channel gets
  "recovered" or "ended" only if a firing message for that alarm reached
  it. If every firing message to the channel failed, the recovery message
  is cancelled. A policy that only listens for `resolved` always sends.
- **Reminders.** A policy with a reminder interval sends a reminder each
  interval while the alarm fires and nobody has acknowledged it.
  Acknowledging or resolving the alarm stops them. If Forge was down for
  several intervals, it sends one reminder, not a backlog. An alarm sends at
  most 50 reminders to a channel.
- **Silences hold messages back.** A delivery whose alarm an active
  [silence](/docs/concepts/silences/) covers is recorded as `suppressed` and not
  sent. When the silence ends, each channel gets at most one message that
  brings it up to date.

Forge keeps finished deliveries for 30 days. The exact statuses, error
classes and retry schedule are in the
[notification payload reference](/docs/reference/notification-payload/).

### What a message contains

A message holds alarm facts only: the event, the alarm's title, severity,
state and times, the rule's id and revision, the resource's kind, id and
name, the observation that fired the alarm, and a link to the alarm in the
console. It never contains secrets, environment values or any URL other
than that console link.

The format is fixed and versioned (`schema_version` `1`). In Slack-style
text, Forge makes names and values inert: markup is escaped, `@` mentions
are broken, and line breaks are removed, so a rule name or a state reported
by a node cannot page a whole chat channel or fake a second alert. An email
carries the same facts; its subject, such as
`[FIRING][critical] Node offline — lab-b`, sorts and filters well, and the
same cleaning keeps a name from adding a header or a line.

### Why channels are stricter than policies

A channel is an outbound destination: it makes the control plane send a
request to a URL, or an email through an SMTP server, that a person chose. That can leak alarm data to the wrong
place, or reach services that only the control plane can reach. A policy
only chooses among channels that already exist. So Forge splits the rights:

| Task | Action | Roles |
| --- | --- | --- |
| Create, change, delete or test a channel | `notification.manage` (MFA) | **Workspace admin**, **Platform admin** |
| Create, change or delete a policy | `alert.manage` | **Operator**, **Workspace admin**, **Platform admin** |
| Read channels, policies and delivery history | `alert.view` | Every role except **Customer** |

These protections apply to every channel:

- **The URL is write-only.** Webhook URLs often carry a token. Forge
  encrypts the URL at rest and never returns it. The console, the API and
  the audit log show only its scheme, host and port.
- **The SMTP password is write-only.** Forge encrypts an email channel's
  password with the channel, like a URL, never returns or logs it, and
  never sends it over an unencrypted connection. The other SMTP settings
  (server, sender, recipients) are visible to everyone who can read the
  channel.
- **The signing secret stays in the vault.** A webhook channel refers to a
  credential vault item; Forge reads the item's password each time it signs
  a message and never copies it into the channel. Choosing the item needs
  the same rights as revealing it: **Platform admin**, a browser session
  (not an API key) and `secret.read` with MFA.
- **Target rules run twice.** Forge checks the URL, or the SMTP host, when
  you save the channel, and checks every address the host name resolves to each time it
  connects. A name that is re-pointed at an internal address after the
  channel was saved is refused. Link-local, cloud metadata, multicast and
  reserved addresses are never called. Loopback and private networks need a
  setting in the server's [`[alerts]` configuration](/docs/reference/configuration/#alerts).
- **No redirects, no proxy.** Forge does not follow a redirect: a `3xx`
  answer is a failed attempt. It verifies TLS certificates and never reads
  the receiver's answer beyond its status code.
- **Encryption is not optional.** An email channel uses STARTTLS or
  implicit TLS. With STARTTLS, a server that stops offering it is not sent
  to: Forge does not fall back to plain text. Unencrypted SMTP needs the
  same operator setting as `http://` webhooks.
- **Failures are recorded as classes.** The delivery history says
  `timeout`, `tls`, `http_status` or `smtp_reply`, with the HTTP status or
  SMTP reply code, never the receiver's text or an error string that could
  quote the URL or the SMTP user name.

### Why SMTP replies decide retries

A mail server says in the first digit of its reply whether to try again:
`4xx` is "not now" (a busy server, greylisting, a full queue) and `5xx` is
"no" (an unknown mailbox, a refused login, a sender the relay does not
accept). Forge follows that: a `4xx` reply is retried on the normal
schedule, a `5xx` reply fails the delivery at once, because sending the same
message again would get the same answer. A connection or TLS failure is
retried like for a webhook.

Delivery history follows the alarm's visibility: a delivery for an alarm on
a resource you cannot `view` stays hidden.

### Not available yet

Provider-specific channels (native Discord, Microsoft Teams, SMS), HTML
email, message templates, grouping, escalation steps, on-call rotations,
per-person notification preferences, recurring quiet hours and
repeating maintenance windows are not available. Forge does not read a receiver's
`Retry-After` header.

To hold notifications back during planned work, see
[Silences and maintenance windows](/docs/concepts/silences/).

## Related

- [Create an alert rule](/docs/guides/alerts/)
- [Send alarm notifications](/docs/guides/alarm-notifications/)
- [Send alerts by email](/docs/guides/email-alerts/)
- [Silences and maintenance windows](/docs/concepts/silences/)
- [Notification payload](/docs/reference/notification-payload/)
- [`forge alerts`](/docs/reference/cli/alerts/)
- [REST API: alerts](/docs/reference/rest-api/#alerts)
