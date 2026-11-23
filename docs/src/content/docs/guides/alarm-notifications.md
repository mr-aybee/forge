---
title: Send alarm notifications
description: Create a notification channel and a policy so alarms reach a webhook or a Slack-compatible chat, then check the delivery history and verify the signature.
sidebar:
  order: 22.5
---

This guide sends alarms to a webhook or a Slack-compatible chat: you create
a channel, test it, and add a policy that picks the alarms. For how
delivery works, see [Alerts](/docs/concepts/alerts/#notifications).

## Before you begin

- To create, change, delete or test a **channel** you need
  `notification.manage` on the workspace: **Workspace admin** or
  **Platform admin**, with an MFA-verified session.
- To create a **policy** you need `alert.manage`: **Operator**,
  **Workspace admin** or **Platform admin**.
- To read channels, policies and **Delivery history**, `alert.view` is
  enough (every role except **Customer**).
- To sign webhook messages you also need the right to use the credential
  vault item that holds the signing secret: **Platform admin**, a browser
  session (not an API key) and `secret.read` with MFA.
- A receiver the Forge server can reach over HTTPS: a webhook endpoint, a
  Slack incoming webhook, a Mattermost incoming webhook or a Discord webhook
  URL that ends in `/slack`. To send email instead, see
  [Send alerts by email](/docs/guides/email-alerts/).
- At least one [alert rule](/docs/guides/alerts/).

The server's operating system does not matter: the Forge server sends the
messages, not the nodes.

:::caution
A channel URL usually contains a token. Forge encrypts it and never shows it
again: the console, the API and the audit log show only the scheme, host and
port. Keep your own copy somewhere safe if you need it later.
:::

## Create a channel

1. Open **Operations → Alerts** and select **Channels**.
2. Select **New channel**.
3. Enter a **Name**, for example `Ops webhook`.
4. Choose the **Kind**: **Webhook** to receive Forge's JSON payload, or
   **Slack-compatible** to receive a text message in a chat.
5. Paste the **URL**. It must start with `https://`.
6. Optional, **Webhook** only: turn on **Sign requests** and choose the
   **Signing secret**, a credential vault item whose password signs each
   message. Create the item first under
   **Administration → Credential vault** (see
   [Credential vault](/docs/guides/credential-vault/)) and give the same password
   to the receiver. Use a random password of at least 16 bytes; Forge refuses
   a shorter one.
7. Select **Save**. If your session is not MFA-verified, Forge asks for a
   code first.

With the CLI, the URL never goes on the command line, where it would stay in
your shell history. Save it in a file with a text editor, for example
`channel-url.txt`, then create the channel and delete the file:

```sh
forge alerts channels create --name "Ops webhook" --kind webhook \
  --url @channel-url.txt --secret-item <vault-item>
rm channel-url.txt
```

`--secret-item` takes the vault item's id or name. Leave it out for an
unsigned channel, and always for `--kind slack`. To read the URL from a
pipe instead, pass `--url @-`.

The answer contains the channel's `id` and its `target`
(`https://hooks.example.com`), not the URL.

## Send a test

1. On **Channels**, select **Send test** on the channel's row.
2. Read the result under the row. **sent** means the receiver answered with
   a `2xx` status. Otherwise the result shows the error, and the HTTP status
   when the receiver answered.

```sh
forge alerts channels test "Ops webhook"
```

The command prints the delivery with its attempt and exits with `10` when
the message was not sent.

A test makes one attempt and does not retry. A webhook receiver gets a
payload with `"event": {"type": "test", …}` and no alarm. A chat shows
`Test notification from Orbyn Forge. No alarm fired.`

## Create a policy

A policy chooses which alarms go to which channels.

1. Select **Policies** on the **Alerts** page, then **New policy**.
2. Enter a **Name**, for example `Critical to ops`.
3. Choose the **Minimum severity**. `critical` sends only critical alarms;
   `warning` sends warning, major and critical ones.
4. Under **Events**, turn on **Fired**, **Resolved** or both. **Resolved**
   also sends a message when Forge stops tracking an alarm without recovery.
5. Under **Channels**, turn on one to ten channels. To email the people
   responsible for the alarm's resource instead, or as well, use
   **Email people** (see
   [Email people by user or role](/docs/guides/email-alerts/#email-people-by-user-or-role)).
6. Optional: limit the policy. Under **Rules**, turn on the rules it covers;
   with none selected it covers every rule. Under **Scope**, choose
   **Nodes** or **Services** and turn on the ones it covers; **All
   resources** covers everything.
7. Optional: under **Reminders**, turn the switch from **Off** to **Every**
   and set an interval from 5 minutes to 7 days. Forge then repeats the
   message while the alarm fires and nobody has acknowledged it.
8. Select **Save**.

With the CLI:

```sh
forge alerts policies create --name "Critical to ops" --min-severity critical \
  --event fired --event resolved --channel "Ops webhook" --repeat 1h
```

`--channel` takes a channel's id or name; repeat it for more channels. Leave
`--repeat` out for no reminders. To limit the policy, add `--rule <rule-id>`
or `--scope servers --scope-id <node-name>`. Every flag is in the
[`forge alerts` reference](/docs/reference/cli/alerts/), and every API field in
the [REST reference](/docs/reference/rest-api/#notifications).

## Verify

1. Make a rule fire. For a `node.agent.status` rule, stop the agent on a
   test node:

   ```sh
   sudo systemctl stop forge-agent
   ```

2. Wait for the alarm under **Alarms**. The message follows within about
   5 seconds.
3. Select **Delivery history**. The delivery for the alarm shows **sent**,
   with one attempt. Select **Details** on the row to see each attempt.
4. Start the agent again (`sudo systemctl start forge-agent`). When the
   alarm resolves, a second delivery with the event `resolved` appears.

With the CLI, list the deliveries, or only the failed ones, and show one
with its attempts:

```sh
forge alerts deliveries list
forge alerts deliveries list --status failed
forge alerts deliveries show <delivery-id>
```

The last command lists every attempt with its status code, error class and
duration.

## Verify the signature on the receiver

A signed `webhook` request carries two extra headers:

```text
X-Forge-Timestamp: 1731750000
X-Forge-Signature: v1=<hex HMAC-SHA256>
```

The signature is HMAC-SHA256 over `<timestamp>.<raw body>`, keyed with the
password of the channel's vault item. On the receiver:

1. Read the raw body bytes. Do not parse and re-serialize the JSON first.
2. Recompute the signature and compare it in constant time.
3. Refuse requests whose timestamp is more than 5 minutes old.
4. Remember `delivery_id` from the signed body and ignore a delivery you already handled:
   Forge delivers at least once.

This Python function uses only the standard library. Pass the secret through
the environment, never in code:

```python
import hashlib
import hmac
import os
import time

SECRET = os.environ["FORGE_WEBHOOK_SECRET"].encode()
MAX_AGE_SECS = 300


def verify(timestamp: str, signature: str, raw_body: bytes) -> bool:
    """True when the request was signed by Forge with SECRET, recently."""
    if not timestamp.isascii() or not timestamp.isdigit():
        return False
    if abs(time.time() - int(timestamp)) > MAX_AGE_SECS:
        return False
    signed = timestamp.encode() + b"." + raw_body
    expected = "v1=" + hmac.new(SECRET, signed, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected.encode(), signature.encode())
```

Call it with the `X-Forge-Timestamp` header, the `X-Forge-Signature` header
and the unparsed body, and answer `401` when it returns `False`.

To check one captured request by hand, save its body to `body.json` without
changing a byte, then recompute the signature with OpenSSL:

```sh
export FORGE_WEBHOOK_SECRET='<signing-secret>'
TIMESTAMP='<value of X-Forge-Timestamp>'
{ printf '%s.' "$TIMESTAMP"; cat body.json; } \
  | openssl dgst -sha256 -hmac "$FORGE_WEBHOOK_SECRET" -r \
  | cut -d' ' -f1
```

The output must equal the hexadecimal part of `X-Forge-Signature` after
`v1=`. Use this for debugging only: a string comparison in a shell is not
constant-time.

## Allow on-premises targets

By default Forge calls only public HTTPS addresses. To reach a receiver on
your own network, a platform operator changes the server's `forge.toml`.

1. On the Forge server, add the setting you need to the `[alerts]` section:

   ```toml
   [alerts]
   # Receivers on private networks, for example https://10.0.0.5/hook
   webhook_allow_private = true
   ```

   | Setting | Allows |
   | --- | --- |
   | `webhook_allow_private` | Private networks such as `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16` and `fc00::/7` |
   | `webhook_allow_http` | `http://` URLs |
   | `webhook_allow_loopback` | `localhost` and loopback addresses on the Forge server itself, for tests and labs |

2. Restart the server:

   ```sh
   sudo systemctl restart forge-server
   ```

3. Save the channel again, or select **Send test**.

:::caution
These settings widen what the control plane can be made to call.
`webhook_allow_private` lets anyone with `notification.manage` send `POST`
requests to services inside your network. `webhook_allow_http` sends alarm
data and the URL's token unencrypted. Turn on only what your receiver
needs. Link-local and cloud metadata addresses such as `169.254.169.254`
stay refused under every setting.
:::

The full list of ranges and defaults is in
[Configuration](/docs/reference/configuration/#alerts).

## Troubleshooting

When you save a channel or a policy:

| Symptom | Cause | Fix |
| --- | --- | --- |
| `403` when you save or test a channel | You lack `notification.manage`, or your session is not MFA-verified | Sign in as **Workspace admin** or **Platform admin** and confirm **Verify it's you** |
| `429` when you select **Send test** twice in a row | A test went to this channel less than 10 seconds ago | Wait a few seconds and send it again |
| `422` "the vault item's password is shorter than 16 bytes" | The signing secret is too short | Rotate the vault item to a random password of at least 16 bytes and give it to the receiver |
| `403` when you choose a signing secret | Using a vault item needs **Platform admin**, a browser session and `secret.read` (MFA) | Have a platform administrator set the secret in the console |
| `422` "only https:// targets are allowed" | The URL starts with `http://` | Use HTTPS, or see [Allow on-premises targets](#allow-on-premises-targets) |
| `422` "private network targets are not allowed" or "loopback targets are not allowed" | The URL names an address on a private network or on the Forge server | See [Allow on-premises targets](#allow-on-premises-targets) |
| `422` "link-local, metadata, multicast and reserved addresses are never called" | The URL names an address Forge refuses under every setting | Use another receiver address |
| `422` "the URL must not carry a user name, a password or a fragment" | The URL has `user:password@` or `#…` | Put the token in the path or the query |
| `422` "only webhook channels sign their messages" | A `slack` channel has a signing secret | Remove the secret: the URL is the secret of a `slack` channel |
| `409` when you delete a channel | A policy still uses it; the message names the policy | Remove the channel from the policy first |
| `409` "the channel is turned off" on **Send test** | The channel is turned off | Turn it on, then test |

When a delivery shows **failed**, look at its error (`last_error`):

| Error | Cause | Fix |
| --- | --- | --- |
| `http_status` with `401` or `403` | The receiver refused the request: wrong token in the URL, or its signature check failed | Compare the secret on both sides; check that the receiver verifies the raw body |
| `http_status` with `404` or `410` | The URL's path is wrong or the webhook was removed | Edit the channel and paste the current URL |
| `http_status` with `301`, `302`, `307` or `308` | The receiver redirects. Forge does not follow redirects | Edit the channel and use the final URL |
| `http_status` with `429` or `5xx`, after several attempts | The receiver stayed busy or broken for every attempt | Fix the receiver. Forge does not send a failed delivery again |
| `timeout` | No answer within 5 seconds to connect or 15 seconds in total | Check firewalls between the Forge server and the receiver; make the receiver answer before it does slow work |
| `connect` | The host name did not resolve, or the connection was refused or reset | Check DNS on the Forge server and that the receiver listens on that port |
| `tls` | The certificate is expired, self-signed, or issued for another name | Install a certificate the Forge server trusts for the name in the URL |
| `blocked_target` | The host name now resolves to an address Forge does not call, or an `[alerts]` setting was turned off after the channel was saved | Point the name at an allowed address, or see [Allow on-premises targets](#allow-on-premises-targets) |
| `secret` | The vault item with the signing secret was deleted, or its password is now shorter than 16 bytes | Edit the channel and choose an existing item with a password of at least 16 bytes |
| `config` | The stored URL cannot be read | Edit the channel and paste the URL again |

When a delivery shows **cancelled**, its detail says why:

| Detail | Cause | Fix |
| --- | --- | --- |
| "the alarm is no longer firing" | The alarm resolved or ended while the message was still waiting or retrying | Nothing: the message was out of date |
| "the alarm was acknowledged" | Someone acknowledged the alarm before a reminder went out | Nothing: reminders stop on acknowledgement |
| "the firing notification was never delivered" | The channel never got the firing message, so Forge sends it no recovery message | Fix the failed firing delivery's cause for the next alarm |
| "the channel is turned off" | The channel was turned off after the delivery was planned | Turn the channel on for later alarms |
| "the channel was deleted" | The channel was deleted after the delivery was planned | Nothing |

Other symptoms:

| Symptom | Cause | Fix |
| --- | --- | --- |
| An alarm fired and **Delivery history** has no row for it | No enabled policy matches: the severity is below the minimum, the rule or resource is outside the policy, the policy does not listen for `fired`, or its channels are turned off | Check the policy and its channels |
| A channel in two policies got one message | Expected: Forge sends one message per alarm event to a channel | Nothing |
| The receiver got the same message twice | Its answer to the first attempt was lost and Forge retried | Drop repeats by `delivery_id` in the body |
| You cannot see a delivery a colleague sees | The alarm is on a resource you cannot `view` | Ask for a role on that resource |

## Next steps

- [Alerts](/docs/concepts/alerts/#notifications): delivery guarantees and the
  security model.
- [Notification payload](/docs/reference/notification-payload/): every header
  and field, statuses and the retry schedule.
- [REST API: Notifications](/docs/reference/rest-api/#notifications): every
  endpoint and field.
- [`forge alerts`](/docs/reference/cli/alerts/): the alert commands of the CLI.
- [Create an alert rule](/docs/guides/alerts/)
