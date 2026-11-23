---
title: Send alerts by email
description: Create an email notification channel that sends alarms through your SMTP server, test it, add a policy, and fix delivery problems.
sidebar:
  order: 22.6
---

This guide sends alarms as email through an SMTP server you
already use: you create an email channel, send a test, and add a policy that
picks the alarms. For how delivery and retries work, see
[Alerts](/docs/concepts/alerts/#notifications).

## Before you begin

- To create, change, delete or test a **channel** you need
  `notification.manage` on the workspace: **Workspace admin** or
  **Platform admin**, with an MFA-verified session.
- To create a **policy** you need `alert.manage`: **Operator**,
  **Workspace admin** or **Platform admin**.
- An SMTP server or relay the Forge server can reach: your mail provider's
  submission service (port `587` with STARTTLS or `465` with TLS) or a relay
  on your network. Ask its administrator for the host name, the port, a user
  name and password if it needs one, and a sender address it accepts.
- At least one [alert rule](/docs/guides/alerts/).

The Forge server sends the email, not the nodes, so the server's operating
system does not matter. The SMTP port must be open from the Forge server.

:::caution
Use a dedicated SMTP account for Forge, with the right to send from one
address only. Forge encrypts the password and never shows it again, but
everyone who can read the channel sees the server, the user name, the sender
and the recipients.
:::

## Create the channel

1. Open **Operations → Alerts** and select **Channels**.
2. Select **New channel**.
3. Enter a **Name**, for example `Ops mail`.
4. Set **Kind** to **Email**.
5. Enter the **SMTP server**, for example `smtp.example.com`. Leave **Port**
   empty to use the usual port of the encryption you choose.
6. Choose the **Encryption**:
   - **STARTTLS (required)**: port `587`. Forge refuses to send when the
     server does not offer STARTTLS.
   - **Implicit TLS (SMTPS)**: port `465`.
   - **None (unencrypted)**: only for a relay on your own network, and only
     when the server configuration allows it. See
     [Use an unencrypted relay](#use-an-unencrypted-relay).
7. Enter the **User name** and **Password** if the server needs a login.
   Leave both empty to send without authentication.
8. Enter **From**, for example `Forge <forge@example.com>`. Many servers
   accept only a sender address that belongs to the account.
9. Enter the recipients under **To**, one address per line, up to 20.
10. Optional: enter a **Subject prefix**, for example `[prod]`, to tell
    environments apart in a mailbox.
11. Select **Save**. If your session is not MFA-verified, Forge asks for a
    code first.

With the CLI, the password never goes on the command line. Save it in a file
with a text editor, for example `smtp-password.txt`, create the channel, then
delete the file:

```sh
forge alerts channels create --name "Ops mail" --kind email \
  --smtp-host smtp.example.com --tls starttls \
  --smtp-user forge@example.com --smtp-password @smtp-password.txt \
  --from "Forge <forge@example.com>" \
  --to ops@example.com --to oncall@example.com \
  --subject-prefix "[prod]"
rm smtp-password.txt
```

To read the password from a pipe instead, pass `--smtp-password @-`. Every
flag is in the [`forge alerts` reference](/docs/reference/cli/alerts/#forge-alerts-channels-create).

To change the password later, select **Edit** on the channel's row and type
the new one in **Replace password**. Leave it empty to keep the stored one.

## Send a test

1. On **Channels**, select **Send test** on the channel's row.
2. Read the result under the row. **sent** means the SMTP server accepted
   the message (`SMTP 250`).
3. Check the recipients' mailboxes for a message with the subject
   `[prod] [TEST] Test notification from Orbyn Forge`. Look in the spam
   folder too.

With the CLI:

```sh
forge alerts channels test "Ops mail"
```

The command exits with `10` when the message was not accepted, and names the
SMTP reply code or the error class.

## Add a policy

A channel sends nothing until a policy uses it.

1. Select **Policies**, then **New policy**.
2. Enter a **Name**, choose the **Minimum severity**, and turn on the
   **Events** you want (**Fired**, **Resolved**).
3. Under **Channels**, turn on `Ops mail`.
4. Select **Save**.

```sh
forge alerts policies create --name "Critical by email" --min-severity critical \
  --event fired --event resolved --channel "Ops mail"
```

[Send alarm notifications](/docs/guides/alarm-notifications/#create-a-policy)
explains every policy option.

## Email people by user or role

Instead of a fixed list, a policy can email the people who are responsible
for the alarm's resource. You need `alert.manage` on the workspace
(**Workspace admin** or **Operator**) and an email channel. How Forge picks
the people is explained in
[Alerts and notifications: How people are chosen](/docs/concepts/alerts/#how-people-are-chosen).

1. Select **Policies**, then **New policy**, or **Edit** on a policy.
2. Under **Email people**, choose the email channel in **Sent through**.
3. Under **Users**, search and turn on the people to email by name. A
   **no email** chip marks a user without an address: they are skipped
   until they set one.
4. Under **Roles**, turn on the roles whose holders should be emailed, for
   example **Operator**. A role means everyone who holds it on the alarm's
   resource, its environment, cluster or workspace.
5. **Channels** can stay empty when the policy only emails people.
6. Select **Save**.

```sh
forge alerts policies create --name "Page operators" --min-severity critical \
  --event fired --event resolved \
  --email-channel "Ops mail" --email-role operator --email-user ana@example.com
```

The channel's own **To** addresses are not added to these emails. Attach the
channel under **Channels** as well if its fixed list should get the alarm
too.

:::note
A named user is emailed only while they may see the alarm (`alert.view` on
its resource). The policy stores user ids and roles; the addresses are
looked up for each alarm.
:::

## What the email looks like

Each email has a designed HTML part and a plain-text part with the same
facts. Mail clients show the HTML part; a client or filter that reads only
text uses the plain-text part.

The HTML part shows:

- The Orbyn Forge header, then a coloured banner with the event: **FIRING**
  in red, orange, amber or blue by severity, **RESOLVED** in green,
  **REMINDER**, **ENDED** and **TEST**.
- The alarm's title and one paragraph in plain words, for example
  `Node lab-b has not reported to Forge for 2 minutes.`
- **What to check**: a few short checks for that kind of alarm, such as
  whether the agent service is running or what is filling the disk.
- A details table: severity, state, resource, node, observed value, rule,
  and when the alarm fired and closed, in UTC and as a duration.
- An **Open alarm in Forge** button. On the alarm page you can acknowledge
  or silence the alarm.
- Why you got the email: the policy and the channel that sent it.

The button and links appear only when the server's `public_url` is set to
the console's `https://` address. The email loads no remote images and
contains no tracking. The Forge mark travels inside the message. The full
layout is in the [notification payload reference](/docs/reference/notification-payload/#html-part).

## Verify

1. Make a rule fire, for example by stopping the agent on a test node
   (`sudo systemctl stop forge-agent`) when you have a `node.agent.status`
   rule.
2. Within a few seconds the recipients get one email with a subject like
   `[prod] [FIRING][critical] Node offline — lab-b`. The body lists the
   alarm, the resource, the observed value and a link to the alarm.
3. Select **Delivery history**. The delivery shows **sent**, and **Details**
   shows the attempt with `SMTP 250`.
4. Start the agent again. When the alarm resolves, a second email arrives
   with `[RESOLVED]` in the subject.

```sh
forge alerts deliveries list --channel "Ops mail"
```

For a policy that emails people, **Delivery history** shows the number of
recipients next to the channel. **Details** lists the addresses (for people
with `notification.manage`) and the people who were skipped, with the
reason.

The email format is described in
[Notification payload: Email](/docs/reference/notification-payload/#email).

## Use an unencrypted relay

Some networks run an internal relay on port `25` without TLS. Forge sends to
it only when an operator allows unencrypted targets, and never with a user
name: a password is never sent in the clear.

1. In the server's configuration, set the `[alerts]` options the relay
   needs (see [Configuration](/docs/reference/configuration/#alerts)):

   ```toml
   [alerts]
   webhook_allow_http = true      # also allows tls = none for email
   webhook_allow_private = true   # the relay is on a private network
   ```

2. Restart the Forge server.
3. Create the channel with **Encryption** set to **None (unencrypted)** and
   no user name, or with `--tls none` in the CLI.

:::caution
`webhook_allow_http` also allows `http://` webhook URLs, and
`webhook_allow_private` lets anyone with `notification.manage` make the
control plane connect to services on your internal network. Turn them on
only when you need them.
:::

## Troubleshooting

When you save the channel:

| Symptom | Cause | Fix |
| --- | --- | --- |
| `422` "tls: `none` sends mail unencrypted" | **Encryption** is **None** and the server does not allow unencrypted targets | Use STARTTLS or implicit TLS, or see [Use an unencrypted relay](#use-an-unencrypted-relay) |
| `422` "a password is never sent without TLS" | **Encryption** is **None** and a user name is set | Choose STARTTLS or implicit TLS, or remove the user name |
| `422` "password: required with a username" | A user name was set and no password is stored | Enter the password |
| `422` "`…` is not an email address" | **From** or a recipient does not parse | Write `name@example.com` or `Name <name@example.com>` |
| `422` "smtp_host: private network targets are not allowed" or "loopback targets are not allowed" | The SMTP server is on a private network or on the Forge server | Allow it with `webhook_allow_private` or `webhook_allow_loopback` in [`[alerts]`](/docs/reference/configuration/#alerts) |
| `422` "smtp_host: link-local, metadata, multicast and reserved addresses are never called" | The host is an address Forge refuses under every setting | Use the relay's real address or name |

When a delivery shows **failed** or keeps retrying:

| Error | Cause | Fix |
| --- | --- | --- |
| `smtp_reply` with `535` | The server refused the login | Check the user name; replace the password with **Edit** → **Replace password** |
| `smtp_reply` with `550`, `553` or `554` | The server refused the sender or a recipient, or the message as spam | Use a sender address the account owns; check the recipients; ask the mail administrator what the server logged |
| `smtp_reply` with `421`, `450` or `451`, retried | The server is busy or greylists new senders | Nothing: Forge retries `4xx` replies on the [retry schedule](/docs/reference/notification-payload/#retry-schedule) |
| `tls`, right away | The server does not offer STARTTLS, or its certificate is expired, self-signed or issued for another name | Use the host name on the certificate; use **Implicit TLS** on port `465` if the server offers it; install a trusted certificate on the relay |
| `tls` with **Implicit TLS** on port `587` | Port `587` expects STARTTLS | Choose **STARTTLS**, or use port `465` |
| `timeout` | The port is filtered, or the server is slow to answer | Open the SMTP port from the Forge server; many cloud providers block port `25` |
| `connect` | The host name does not resolve, or nothing listens on the port | Check DNS on the Forge server and the port |
| `blocked_target` | The host now resolves to an address Forge does not call, or an `[alerts]` setting was turned off | Point the name at an allowed address, or change the setting |
| `secret` | The stored password cannot be read | Edit the channel and enter the password again |

Other symptoms:

| Symptom | Cause | Fix |
| --- | --- | --- |
| The delivery shows **sent** but nobody got the email | The server accepted it and then filtered it, or it went to spam | Check the spam folder and the server's log for the `Message-ID` `<delivery-id@…>` |
| A recipient got the same email twice | The server's answer to the first attempt was lost and Forge retried | Filter repeats by `Message-ID`: it is the same on every retry |
| Out-of-office replies reach the sender address | The replying server ignores `Auto-Submitted: auto-generated` | Use a sender address that discards replies |

When you email people by user or role:

| Symptom | Cause | Fix |
| --- | --- | --- |
| A **no email** chip next to a user, or `warnings.users_without_email` in the API | The user has no email address | The user sets one under **Profile** → **Your account**, or an administrator sets it on the user |
| **Details** lists a user as skipped with `no_email` | The account had no address when the alarm fired | Set the address; the next alarm event reaches them |
| Skipped with `no_access` | The user may not see the alarm's resource (no `alert.view` on it) | Give them a role on the resource, its environment or the workspace; or remove them from the policy |
| Skipped with `disabled` | The account is disabled | Enable the account, or remove the user from the policy |
| The delivery is **cancelled** with `no recipients` | Every person was skipped, or nobody holds the roles on that resource | Check **Details** for the reasons; grant the role where the alarm's resource lives |
| `422` "email_channel: required to email users or roles" | Users or roles are set without **Sent through** | Choose the email channel |
| `422` "is not an email channel" | **Sent through** names a webhook or Slack channel | Choose an email channel |
| `422` "user … has no role in this workspace" | The user holds no role on the platform, the workspace or anything in it | Grant the user a role first |
| A role holder on another environment got no email | Roles count on the alarm's resource and above it, not on siblings | Grant the role on the environment or workspace the alarm belongs to |

## Next steps

- [Send alarm notifications](/docs/guides/alarm-notifications/): webhooks,
  Slack-compatible chats and every policy option.
- [Silence alarms](/docs/guides/silence-alarms/): hold messages back during
  planned work.
- [Notification payload: Email](/docs/reference/notification-payload/#email):
  headers, subject and body.
- [REST API: Notifications](/docs/reference/rest-api/#notifications): the
  `email` channel fields.
- [Alerts](/docs/concepts/alerts/#notifications): the delivery and security
  model.
