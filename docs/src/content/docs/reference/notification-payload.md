---
title: Notification payload
description: The request or email Forge sends to a notification channel, with every header and payload field, the signature scheme, the email format, delivery statuses, error classes and the retry schedule.
sidebar:
  order: 41
---

This page lists exactly what Forge sends to a notification channel and how
it records the result. For the model behind it, see
[Alerts](/docs/concepts/alerts/#notifications). For the task, see
[Send alarm notifications](/docs/guides/alarm-notifications/).

The formats are fixed. Message templates are not available.

## Request

For a `webhook` or `slack` channel, Forge sends one `POST` to the channel's
URL for each attempt. An `email` channel gets an [email](#email) instead.

| Property | Value |
| --- | --- |
| Method | `POST` |
| Body | `webhook` channel: the [JSON payload](#webhook-payload). `slack` channel: [`{"text": "…"}`](#slack-compatible-text) |
| Connect timeout | 5 seconds |
| Total timeout | 15 seconds, answer headers included |
| TLS | Certificate and host name are verified |
| Redirects | Not followed. A `3xx` answer fails the delivery |
| Proxy | Not used, even when proxy environment variables are set |
| Answer | Only the status code is read. `2xx` means sent |

### Headers

| Header | Sent | Value |
| --- | --- | --- |
| `Content-Type` | Always | `application/json` |
| `User-Agent` | Always | `Orbyn-Forge-Alerts/<version>` |
| `X-Forge-Event` | Always | `fired`, `resolved`, `ended`, `reminder` or `test` |
| `X-Forge-Delivery` | Always | The delivery id. The same on every retry of one delivery |
| `X-Forge-Timestamp` | `webhook` channels with a signing secret | Time of signing, in seconds since the Unix epoch. New on every attempt |
| `X-Forge-Signature` | `webhook` channels with a signing secret | `v1=<hex>`, see [Signature](#signature) |

## Signature

A `webhook` channel with a signing secret (`secret_item`) signs every
request. `slack` channels are never signed: their URL is the secret.

| Part | Definition |
| --- | --- |
| Key | The password of the channel's credential vault item, as UTF-8 bytes. At least 16 bytes: Forge refuses a shorter one when you save the channel, and fails the delivery (`secret`) if the item's password is shortened later |
| Signed text | `<X-Forge-Timestamp>.<raw request body>`: the timestamp in decimal, one `.`, then the body bytes exactly as received |
| Algorithm | HMAC-SHA256 |
| Header | `X-Forge-Signature: v1=<64 lowercase hexadecimal characters>` |

To verify a request:

1. Read the raw body bytes before any JSON parsing.
2. Compute HMAC-SHA256 over `<timestamp>.<raw body>` with the shared secret.
3. Compare the result with the value after `v1=` in constant time.
4. Refuse a timestamp more than 5 minutes from your clock.
5. Drop a delivery whose `delivery_id` you have already processed. Forge
   delivers at least once, and a retry is signed again with a new timestamp.

:::caution
The signature covers the timestamp and the body, nothing else. Act only on
what the body says: read the event from `event.type` and the delivery from
`delivery_id`. `X-Forge-Event` and `X-Forge-Delivery` repeat those values
for routing and logs, are not signed, and can be changed by anyone who can
replay a request.
:::

See [Verify the signature on the receiver](/docs/guides/alarm-notifications/#verify-the-signature-on-the-receiver)
for example code.

## Webhook payload

`schema_version` is `1`. It changes only when a field is removed or changes
meaning. New optional fields can appear without a version change, so ignore
fields you do not know. Times are epoch milliseconds.

```json
{
  "schema_version": 1,
  "delivery_id": "<delivery-id>",
  "workspace_id": "<workspace-id>",
  "event": { "type": "fired", "is_resolved": false, "occurrence": 0, "at": 1731750000000 },
  "alarm": {
    "id": "<alarm-id>",
    "title": "Node offline",
    "severity": "critical",
    "state": "firing",
    "fired_at": 1731749990000,
    "acknowledged": false
  },
  "rule": { "id": "<rule-id>", "revision": 1 },
  "resource": { "kind": "server", "id": "<server-id>", "name": "web-01" },
  "observation": {
    "subject": "node.agent.status",
    "value": { "type": "state", "value": "offline" }
  },
  "links": { "alarm": "https://forge.example.com/alerts?alarm=<alarm-id>" }
}
```

| Field | Type | Present | Meaning |
| --- | --- | --- | --- |
| `schema_version` | integer | Always | `1` |
| `delivery_id` | id | Always | The delivery. Equal to `X-Forge-Delivery`; the same on every retry |
| `workspace_id` | id | Always | The workspace of the channel |
| `event.type` | string | Always | `fired`, `resolved`, `ended`, `reminder` or `test` |
| `event.is_resolved` | boolean | Always | `true` only for `resolved`. `false` for `ended`: tracking stopped without recovery evidence |
| `event.occurrence` | integer | Always | `0`, or the number of the reminder (from `1`) |
| `event.at` | integer, epoch ms | Always | When Forge planned the delivery |
| `alarm.id` | id | Alarm events | The alarm |
| `alarm.title` | string | Alarm events | The rule's name when the alarm fired |
| `alarm.severity` | string | Alarm events | `info`, `warning`, `major` or `critical` |
| `alarm.state` | string | Alarm events | `firing`, `resolved` or `ended`, when the attempt was made |
| `alarm.fired_at` | integer, epoch ms | Alarm events | When the alarm fired |
| `alarm.resolved_at` | integer, epoch ms | When the alarm is resolved or ended | When it resolved, or when tracking ended |
| `alarm.end_reason` | string | When the alarm ended | Why tracking ended, for example `the rule was deleted` |
| `alarm.acknowledged` | boolean | Alarm events | Whether someone acknowledged the alarm |
| `alarm.acknowledged_at` | integer, epoch ms | When acknowledged | When it was acknowledged |
| `rule.id` | id | Alarm events | The alert rule |
| `rule.revision` | integer | Alarm events | The rule revision that fired the alarm |
| `resource.kind` | string | Alarm events | `server` or `service` |
| `resource.id` | id | Alarm events | The resource |
| `resource.name` | string | When Forge can read the resource | A node's display name or host name; a service's name |
| `resource.server_id` | id | Service instances | The node the service instance runs on |
| `observation.subject` | string | Alarm events | The rule's subject, such as `node.agent.status` |
| `observation.value` | object | Alarm events | The value that fired the alarm: `{"type": "state", "value": "<state>"}` or `{"type": "num", "value": <number>}` |
| `links.alarm` | string | Alarm events | The alarm in the console: `<public address>/alerts?alarm=<alarm-id>`. The [public address](/docs/reference/configuration/#public-address) is the runtime setting, else `public_url` |
| `message` | string | Test messages only | `Test notification from Orbyn Forge. No alarm fired.` |
| `after_silence.silence_id` | id | Only on a message sent because a silence ended | The silence that held this message back. See [After a silence](#after-a-silence) |

"Alarm events" are `fired`, `resolved`, `ended` and `reminder`. A test
message has no `alarm`, `rule`, `resource`, `observation` or `links`:

```json
{
  "schema_version": 1,
  "delivery_id": "<delivery-id>",
  "workspace_id": "<workspace-id>",
  "event": { "type": "test", "is_resolved": false, "occurrence": 0, "at": 1731750000000 },
  "message": "Test notification from Orbyn Forge. No alarm fired."
}
```

A payload never contains secrets, environment values or a URL other than
`links.alarm`.

### Events

| `event.type` | Sent when | Policy event that selects it |
| --- | --- | --- |
| `fired` | The alarm opens | `fired` |
| `reminder` | The alarm still fires, nobody acknowledged it, and the policy's `repeat_interval_secs` passed again | `fired`, with `repeat_interval_secs` |
| `resolved` | Valid evidence showed recovery | `resolved` |
| `ended` | Tracking stopped without recovery: the rule was changed, disabled or deleted, or the node was removed | `resolved` |
| `test` | Someone sent a test message to the channel | None |

### After a silence

While an active [silence](/docs/concepts/silences/) covers an alarm, its
deliveries are `suppressed` and not sent. When the silence ends, each
channel gets at most one message that brings it up to date. That message
has the usual `event.type` (`fired`, `reminder`, `resolved` or `ended`), a
new `delivery_id`, and an `after_silence` object:

```json
{
  "schema_version": 1,
  "delivery_id": "<delivery-id>",
  "event": { "type": "fired", "is_resolved": false, "occurrence": 0, "at": 1731760000000 },
  "alarm": { "id": "<alarm-id>", "title": "Node offline", "severity": "warning", "state": "firing" },
  "after_silence": { "silence_id": "<silence-id>" }
}
```

The example leaves out the fields that every alarm message carries.
`event.at` is when the silence was settled, not when the alarm fired; read
`alarm.fired_at` for that. An alarm that fired and resolved inside the
silence sends nothing.

## Slack-compatible text

A `slack` channel receives `{"text": "<lines>"}` with the lines joined by
newlines:

```text
ALERT [critical] Node offline
Resource: server web-01
Observed: node.agent.status = offline
https://forge.example.com/alerts?alarm=<alarm-id>
```

| Line | Content |
| --- | --- |
| 1 | `<head> [<severity>] <alarm title>` |
| 2 | `Resource: <kind> <name>`. The resource's id when Forge has no name |
| 3 | `Observed: <subject> = <value>` |
| 4, some events | `reminder`, not acknowledged: `Nobody has acknowledged this alarm.` `ended`: `Tracking ended without recovery: <reason>` |
| Next, after a silence | `Sent now because a silence that held this back has ended.` |
| Last | The link to the alarm in the console |

| Event | `<head>` |
| --- | --- |
| `fired` | `ALERT` |
| `reminder` | `STILL FIRING` |
| `resolved` | `RECOVERED` |
| `ended` | `ENDED` |

A test message is the single line
`Test notification from Orbyn Forge. No alarm fired.`

Forge makes every name and value in the text inert before sending:

| Input | Becomes |
| --- | --- |
| `&`, `<`, `>` | `&amp;`, `&lt;`, `&gt;`, so `<!channel>` and `<https://…\|label>` are plain text |
| `@` | `@` followed by a zero-width space, so `@everyone` and `@here` do not mention anyone |
| Control characters, line breaks and tabs | Removed |
| Invisible and direction-changing characters: line and paragraph separators (U+2028, U+2029), bidirectional overrides and isolates, zero-width characters | Removed |
| `://` | `:` followed by a zero-width space, then `//`, so a name or a value never becomes a link. The only link in a message is the console link Forge adds |
| More than 200 characters | Cut to 200 |

## Email

An `email` channel gets one email per attempt, with a plain-text part and an
HTML part ([Parts](#parts)), sent over SMTP to
every recipient of the channel at once. A policy that emails people by user
or role sends a separate email through its email channel to the addresses
resolved for that alarm, as blind copies (see [Headers](#headers)).

| Property | Value |
| --- | --- |
| Connection | To the channel's `smtp_host` and `smtp_port`. Forge resolves the host itself and refuses it when any of its addresses breaks the target rules (`blocked_target`), then connects to a checked address |
| Encryption | `starttls`: `STARTTLS` is required; a server that does not offer it fails the attempt as `tls` and nothing is sent in the clear. `tls`: TLS from the first byte. `none`: no encryption |
| Certificate | Verified against `smtp_host` with the operating system's trusted roots |
| Authentication | With `username`: `AUTH` with the stored password (`PLAIN` or `LOGIN`, as the server offers). Never over an unencrypted connection |
| `EHLO` name | The host of the server's `public_url`, or `localhost` |
| Timeouts | 10 seconds for each SMTP command, connecting included; 40 seconds for the whole message |
| Result | The server's reply code is kept in `last_http_status` (`250`). `2xx` means sent |

### Headers

| Header | Value |
| --- | --- |
| `From` | The channel's `from` |
| `To` | The channel's recipients. Left out of an email to a policy's people: they are envelope recipients only (blind copies), so nobody sees the other addresses |
| `Subject` | See [Subject](#subject) |
| `Message-ID` | `<delivery-id@public-host>`: the same on every retry of a delivery, so a mailbox or filter can drop a repeat. `orbyn-forge.invalid` replaces the host when `public_url` has no host name |
| `Date` | When the attempt was made |
| `Auto-Submitted` | `auto-generated`, so out-of-office replies are not sent back |
| `Content-Type` | `multipart/alternative`. See [Parts](#parts) |

### Parts

```diagram
alt: MIME structure of an alert email
tree:
  - label: "`multipart/alternative`"
    children:
      - { label: "`text/plain; charset=utf-8`", note: the body below }
      - label: "`multipart/related`"
        children:
          - { label: "`text/html; charset=utf-8`", note: the designed message }
          - { label: "`image/png`", note: "the Orbyn Forge mark, inline · `Content-ID: <forge-mark@orbyn-forge>` · `Content-Disposition: inline`" }
```

| Part | Content |
| --- | --- |
| `text/plain` | The [body](#body). A mail client that shows no HTML, or a filter that reads text, uses this part. Long lines are sent quoted-printable |
| `text/html` | The same facts, laid out for reading. See [HTML part](#html-part) |
| `image/png` | The brand mark, 128 × 128 pixels (about 5 KB), shown at 32 pixels in the header. It is built into the server, so the message loads no remote image |


### Subject

```text
[prod] [FIRING][critical] Node offline — lab-b
```

`<subject_prefix> [<marker>][<severity>] <alarm title> — <resource name>`.
Without a prefix the subject starts at `[`. A message after a silence ends
in ` (after silence)`. A test message's subject is
`[TEST] Test notification from Orbyn Forge`, after the prefix.

| Event | `<marker>` |
| --- | --- |
| `fired` | `FIRING` |
| `reminder` | `REMINDER` |
| `resolved` | `RESOLVED` |
| `ended` | `ENDED` |
| `test` | `TEST` |

### Body

The body holds the same facts as the [Slack-compatible text](#slack-compatible-text):

```text
An alarm is firing.

Alarm:    Node offline
Severity: critical
State:    firing
Resource: server lab-b
Observed: node.agent.status = offline
Fired at: 2026-11-16T09:30:00Z

Open the alarm: https://forge.example.com/alerts?alarm=<alarm-id>

Alarm <alarm-id>, delivery <delivery-id>
```

| Line | Content |
| --- | --- |
| 1 | `An alarm is firing.`, `An alarm is still firing.` (`reminder`), `An alarm recovered.` (`resolved`) or `Tracking of an alarm ended without recovery.` (`ended`) |
| `Alarm`, `Severity`, `State` | The alarm's title, severity and state |
| `Resource` | `<kind> <name>`. The resource's id when Forge has no name |
| `Observed` | `<subject> = <value>` |
| `Fired at`, `Closed at` | UTC, RFC 3339. `Closed at` only once the alarm resolved or ended |
| Some events | `reminder`, not acknowledged: `Nobody has acknowledged this alarm.` `ended`: `Tracking ended without recovery: <reason>`. After a silence: `Sent now because a silence that held this back has ended.` |
| `Open the alarm` | The link to the alarm in the console. Uses the [public address](/docs/reference/configuration/#public-address). Left out when there is none |
| Last | The alarm's and the delivery's ids |

A test message's body says `Test notification from Orbyn Forge. No alarm
fired.` and names the delivery.

### HTML part

The HTML part uses tables and inline styles only, at most 600 pixels wide,
with no external style sheet, font, script or remote image. It shows, from
top to bottom:

| Section | Content |
| --- | --- |
| Header | The Orbyn Forge mark and name on dark blue |
| State banner | The event in capitals, coloured as in the table below. A message after a silence adds `sent after a silence ended` |
| Title | The alarm's title |
| Explanation | One paragraph in plain words, chosen by the observation subject (see below) |
| Notes | `reminder`, not acknowledged: `Nobody has acknowledged this alarm yet.` After a silence: a note that a silence held the message back |
| What to check | Two to four short checks for the subject family |
| Details | **Severity**, **State** (with acknowledged or not while firing), **Resource** (name and kind), **Node** (when the resource runs on a node), **Observed** (`subject = value`, `%` for percentages), **Rule** (id and revision), **Fired at** and **Closed at** (UTC, with how long ago or how long it lasted) |
| Links | **Open alarm in Forge**, and links to acknowledge or silence it from the alarm page and to the **Silences** tab. Only when the [public address](/docs/reference/configuration/#public-address) is an absolute `http` or `https` URL |
| Footer | The policy and the channel that sent the message, the alarm's and the delivery's ids |

| Event | Banner | Colour |
| --- | --- | --- |
| `fired` | `FIRING` | By severity: `critical` red, `major` orange, `warning` amber, `info` blue |
| `reminder` | `REMINDER · STILL FIRING` | By severity, as `fired` |
| `resolved` | `RESOLVED` | Green |
| `ended` | `ENDED · NO RECOVERY SEEN` | Grey |
| `test` | `TEST` | Violet |

| Observation subject | Explanation, for example |
| --- | --- |
| `node.agent.status` | `Node lab-b has not reported to Forge for 2 minutes. Its agent status is offline, so Forge cannot see or manage it.` |
| `node.cpu.percent`, `node.mem.used_percent`, `node.disk.used_percent` | `Node lab-b is reporting high CPU use: 97.5%. This started 4 minutes ago.` |
| Other `node.*` | `Node lab-b is reporting <subject>: <value>.` |
| `health.*` | `The health check "web" on node lab-b is failing (observed: failing).` |
| `app.*`, `package.*`, `tls.*` | A sentence about the service, the packages or the certificate, with the subject and value |
| Anything else | `A rule's condition is met on <resource> (<subject> = <value>).` |
| Any subject, `resolved` | `<Resource> is back to normal. The alarm "<title>" recovered after <duration>.` |
| Any subject, `ended` | Forge stopped tracking the alarm without seeing it recover, with the reason |

Every name and value in the HTML part is first made inert as in the
plain-text part, then HTML-escaped, so `<`, `>`, `&` and quotes show as
text. The only links point to the console.

Names and values in the subject and the body are made inert: control
characters, line breaks, tabs and invisible or direction-changing characters
are removed, `://` gets a zero-width space so a value never becomes a link,
and a value longer than 200 characters is cut. A rule name cannot add a
header or a line. In the plain-text part `&`, `<` and `>` stay as they are. Nothing from the vault and no SMTP setting is ever part of a message.

## Delivery statuses

| Status | Meaning | Final |
| --- | --- | --- |
| `pending` | Waiting for its first or next attempt (`next_attempt_at`) | No |
| `sending` | An attempt is running. An attempt that never reports back is taken up again after 60 seconds | No |
| `sent` | The receiver answered `2xx` (an SMTP server: `250`) | Yes |
| `failed` | Refused for good, or out of attempts. `last_error` says why | Yes |
| `cancelled` | Not sent because it no longer applies. `detail` says why | Yes |
| `suppressed` | Not sent because an active [silence](/docs/concepts/silences/) covered the alarm. `silence_id` names the silence; `detail` is `silenced: <reason>` (the reason cut to 200 characters). When the silence ends, at most one [message per channel](#after-a-silence) follows | Yes |

Forge deletes final deliveries and their attempts 30 days after they were
created.

## Error classes

`last_error` on a delivery and `error` on an attempt hold one of these
classes. Forge never stores the receiver's answer text.

| Class | Meaning | Retried |
| --- | --- | --- |
| `timeout` | No answer within the connect or total timeout | Yes |
| `connect` | The name did not resolve, or the connection was refused or reset | Yes |
| `tls` | The TLS handshake or the certificate check failed, or an SMTP server did not offer the `STARTTLS` that `tls = starttls` requires | Yes |
| `http_status` | The receiver answered outside `2xx`. The status is in `last_http_status` | `408`, `425`, `429` and `500`–`599`: yes. Every other status, `3xx` included: no |
| `blocked_target` | The URL, or an address its host name resolves to, is one Forge does not call under the current [`[alerts]` settings](/docs/reference/configuration/#alerts) | No |
| `secret` | The signing secret cannot be read | Vault item deleted or in another workspace: no. Vault unreadable at that moment: yes |
| `config` | The stored channel configuration cannot be used | No |
| `smtp_reply` | The SMTP server refused the message, its sender, a recipient or the login. The reply code is in `last_http_status` | `4xx` (for example `421`, `450`, `451`, greylisting): yes. `5xx` (for example `535` login refused, `550` mailbox unavailable): no |

## Cancel reasons

| `detail` | Cause |
| --- | --- |
| `the alarm is no longer firing` | A `fired` or `reminder` delivery was still waiting when the alarm resolved or ended |
| `the alarm was acknowledged` | A `reminder` was still waiting when someone acknowledged the alarm |
| `the firing notification was never delivered` | A `resolved` or `ended` delivery whose channel never received a firing message for that alarm: each one failed or was cancelled |
| `the firing notification was silenced, so there is nothing to recover from` | A `resolved` or `ended` delivery whose channel's firing message for that alarm was `suppressed` and never sent |
| `the channel is turned off` | The channel was turned off after the delivery was planned |
| `the channel was deleted` | The channel was deleted after the delivery was planned |
| `the alarm no longer exists` | The alarm was removed before the message was sent |
| `the test was interrupted` | A test message whose request ended before it was sent (for example, the server restarted). Send the test again |
| `no recipients` | A policy's delivery to people (`email_users`, `email_roles`) found nobody to email when it was planned: every person was skipped (no email address, no access to the alarm, or a disabled account). The delivery's `recipients.skipped` lists them by id |

A `resolved` or `ended` delivery waits in `pending` while a firing message
for the same alarm and channel is still being retried, and is checked again
every 15 seconds.

## Retry schedule

After a retryable failure, Forge waits between half of a ceiling and the
whole ceiling. The ceiling starts at `delivery_retry_base_secs`, doubles
after each attempt and stops at `delivery_retry_max_secs`. The exact wait
within that range is fixed per delivery, so deliveries that failed together
do not all retry at the same moment.

With the defaults (`delivery_attempts = 6`, `delivery_retry_base_secs = 30`,
`delivery_retry_max_secs = 900`):

| After attempt | Wait before the next attempt |
| --- | --- |
| 1 | 15–30 seconds |
| 2 | 30–60 seconds |
| 3 | 1–2 minutes |
| 4 | 2–4 minutes |
| 5 | 4–8 minutes |
| 6 | None: the delivery is `failed` |

Forge sends due deliveries every 5 seconds, so an attempt can start up to
5 seconds after its time. A test message gets one attempt and no retry.
Forge does not read a `Retry-After` header.

Each channel is its own queue. Forge takes at most two due deliveries of a
channel into a pass and sends them one after the other, and stops with that
channel for the pass at its first timeout. After two timeouts in a row it
leaves the channel alone for 60 seconds. A receiver that does not answer
therefore slows its own channel, not the others. The count is kept in
memory and starts again when the server restarts.

## Related

- [Alerts](/docs/concepts/alerts/#notifications)
- [Send alarm notifications](/docs/guides/alarm-notifications/)
- [REST API: Notifications](/docs/reference/rest-api/#notifications)
- [Configuration: Alerts](/docs/reference/configuration/#alerts)
