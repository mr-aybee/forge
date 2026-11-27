---
title: forge alerts
description: List and manage alert rules, alarms, notification channels, notification policies, the delivery history and silences.
sidebar:
  order: 20
---

`forge alerts` works with [alert rules and alarms](/docs/concepts/alerts/) and
with their [notifications](/docs/concepts/alerts/#notifications): channels,
policies and deliveries, and with [silences](/docs/concepts/silences/). For the tasks, see
[Create an alert rule](/docs/guides/alerts/) and
[Send alarm notifications](/docs/guides/alarm-notifications/). Every command
takes the [global flags](/docs/reference/cli/#global-flags).

## Commands

| Command | What it does |
| --- | --- |
| [`forge alerts alarms`](#forge-alerts-alarms) | Lists alarms, newest first |
| [`forge alerts alarm`](#forge-alerts-alarm) | Shows one alarm and its timeline |
| [`forge alerts ack`](#forge-alerts-ack) | Acknowledges an alarm |
| [`forge alerts unack`](#forge-alerts-unack) | Removes an alarm's acknowledgement |
| [`forge alerts note`](#forge-alerts-note) | Adds a note to an alarm's timeline |
| [`forge alerts rules`](#forge-alerts-rules) | Lists alert rules |
| [`forge alerts rule`](#forge-alerts-rule) | Shows one rule |
| [`forge alerts subjects`](#forge-alerts-subjects) | Lists the subjects a rule can watch |
| [`forge alerts templates`](#forge-alerts-templates) | Lists ready-made rules |
| [`forge alerts create-rule`](#forge-alerts-create-rule) | Creates a rule from a JSON document or a template |
| [`forge alerts update-rule`](#forge-alerts-update-rule) | Replaces a rule from a JSON document |
| [`forge alerts delete-rule`](#forge-alerts-delete-rule) | Deletes a rule |
| [`forge alerts channels list`](#forge-alerts-channels-list) | Lists notification channels |
| [`forge alerts channels show`](#forge-alerts-channels-show) | Shows one channel |
| [`forge alerts channels create`](#forge-alerts-channels-create) | Creates a channel from flags |
| [`forge alerts channels update`](#forge-alerts-channels-update) | Changes the fields of a channel that the flags name |
| [`forge alerts channels delete`](#forge-alerts-channels-delete) | Deletes a channel |
| [`forge alerts channels test`](#forge-alerts-channels-test) | Sends a test message and shows the result |
| [`forge alerts policies list`](#forge-alerts-policies-list) | Lists notification policies |
| [`forge alerts policies show`](#forge-alerts-policies-show) | Shows one policy |
| [`forge alerts policies create`](#forge-alerts-policies-create) | Creates a policy from flags |
| [`forge alerts policies update`](#forge-alerts-policies-update) | Changes the fields of a policy that the flags name |
| [`forge alerts policies delete`](#forge-alerts-policies-delete) | Deletes a policy |
| [`forge alerts deliveries list`](#forge-alerts-deliveries-list) | Lists notification deliveries, newest first |
| [`forge alerts deliveries show`](#forge-alerts-deliveries-show) | Shows one delivery with its attempts |
| [`forge alerts silences list`](#forge-alerts-silences-list) | Lists silences and maintenance windows, latest start first |
| [`forge alerts silences show`](#forge-alerts-silences-show) | Shows one silence |
| [`forge alerts silences create`](#forge-alerts-silences-create) | Creates a silence or a maintenance window |
| [`forge alerts silences extend`](#forge-alerts-silences-extend) | Moves a silence's end later |
| [`forge alerts silences end`](#forge-alerts-silences-end) | Ends a silence now |

Exit codes for every command: `0` success, `2` invalid input (a `400` or
`422` answer, a flag value that is refused, or a JSON file that does not
parse), `3` not signed in, `4` the action is denied, `5` the action needs
MFA, `6` no such alarm, rule, channel, policy, delivery or silence (or you cannot see
it), `7` a conflict (for example a channel that a policy still uses), `9`
the server is unreachable, `10` a test message that was not delivered.

### forge alerts alarms

Lists alarms of the workspace on nodes you can see, newest first. Needs
`alert.view` on the workspace.

**Synopsis**

```text
forge alerts alarms [--active] [--rule <RULE>] [--node <NODE>] [--limit <LIMIT>]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--active` | flag | off | no | Only firing alarms | `--active` |
| `--rule <RULE>` | rule id | — | no | Only this rule's alarms | `--rule <rule-id>` |
| `--node <NODE>` | node id, host name or display name | — | no | Only this node's alarms | `--node web-01` |
| `--workspace <WORKSPACE>` | workspace id | none | no | Only this workspace's alarms | `--workspace <workspace-id>` |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

**Examples**

```sh
# Alarms that are firing now
forge alerts alarms --active
# Alarms on one node
forge alerts alarms --node <node-name>
# Every alarm of one rule on one page
forge alerts alarms --rule <rule-id> --all
```

**Output**

```text
$ forge alerts alarms --active
ID                                    STATE   SEVERITY  TITLE         NODE    FIRED (UTC)          ACKNOWLEDGED
01a1daee-bf4d-75db-9c82-103e266e90d9  firing  critical  Node offline  web-01  2026-11-14 16:40:42  no

$ forge alerts alarms
ID                                    STATE     SEVERITY  TITLE         NODE    FIRED (UTC)          ACKNOWLEDGED
01a1daee-bf4d-75db-9c82-103e266e90d9  resolved  critical  Node offline  web-01  2026-11-14 16:40:42  yes
```

`NODE` is the node's display name or host name; `FIRED (UTC)` and
`ACKNOWLEDGED` are formatted for the table. With `-o json` each alarm has
the [REST alarm fields](/docs/reference/rest-api/#alerts) (`fired_at` and
`acknowledged_at` in epoch milliseconds).


### forge alerts alarm

Shows one alarm with its timeline (`events`). Needs `alert.view` on the
workspace and `view` on the alarm's node.

**Synopsis**

```text
forge alerts alarm <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | alarm id | — | yes | The alarm | `<alarm-id>` |

**Examples**

```sh
# One alarm with its timeline
forge alerts alarm <alarm-id>
# The same alarm as JSON
forge alerts alarm <alarm-id> -o json
```

**Output**

An alarm that was acknowledged and then resolved (ids shortened):

```text
$ forge alerts alarm <alarm-id>
{
  "acknowledged_at": 1794673327375,
  "acknowledged_by": { "id": "<user-id>", "type": "user" },
  "events": [
    { "at": 1794673325505, "kind": "fired",
      "detail": { "revision": 1, "value": { "type": "state", "value": "offline" } } },
    { "at": 1794673327375, "kind": "acknowledged",
      "actor": { "id": "<user-id>", "type": "user" }, "detail": { "note": "Rebooting web-01" } },
    { "at": 1794673370488, "kind": "resolved",
      "detail": { "value": { "type": "state", "value": "online" } } }
  ],
  "fired_at": 1794673325505,
  "fired_value": { "type": "state", "value": "offline" },
  "resolved_at": 1794673370488,
  "resource": { "id": "<server-id>", "kind": "server" },
  "rule_revision": 1,
  "severity": "critical",
  "state": "resolved",
  "subject": "node.agent.status",
  "title": "Node offline"
}
```

The output also has `id`, `rule_id`, `workspace_id` and `fingerprint`.
Each event also repeats `alarm_id`.


### forge alerts ack

Acknowledges an alarm: it records that someone is working on it. The alarm
keeps firing until it recovers. Needs `alert.manage`; audited as
`alert.alarm.acknowledge`.

**Synopsis**

```text
forge alerts ack <ID> [--note <NOTE>]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | alarm id | — | yes | The alarm | `<alarm-id>` |
| `--note <NOTE>` | string | empty | no | A note for the timeline, at most 2000 characters | `--note "Rebooting the node"` |

**Examples**

```sh
# Acknowledge an alarm
forge alerts ack <alarm-id>
# Acknowledge it with a note for the timeline
forge alerts ack <alarm-id> --note "Rebooting <node-name>"
```

**Output**

The alarm after the change, with `acknowledged_by` and `acknowledged_at`
set, as in [`forge alerts alarm`](#forge-alerts-alarm) without `events`.

### forge alerts unack

Removes an alarm's acknowledgement. Needs `alert.manage`; audited as
`alert.alarm.unacknowledge`.

**Synopsis**

```text
forge alerts unack <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | alarm id | — | yes | The alarm | `<alarm-id>` |

**Examples**

```sh
# Remove an alarm's acknowledgement
forge alerts unack <alarm-id>
```

**Output**

The alarm after the change, without `acknowledged_by` and
`acknowledged_at`.

### forge alerts note

Adds a note to an alarm's timeline. Needs `alert.manage`; audited as
`alert.alarm.note`.

**Synopsis**

```text
forge alerts note <ID> <TEXT>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | alarm id | — | yes | The alarm | `<alarm-id>` |
| `<text>` | string | — | yes | The note, 1 to 2000 characters | `"Disk replaced"` |

**Examples**

```sh
# Add a note to an alarm's timeline
forge alerts note <alarm-id> "Disk replaced"
```

**Output**

```text
added  true
```

### forge alerts rules

Lists the workspace's alert rules. Needs `alert.view` on the workspace.

**Synopsis**

```text
forge alerts rules
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--workspace <WORKSPACE>` | workspace id | none | no | Only this workspace's rules | `--workspace <workspace-id>` |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

**Examples**

```sh
# Every alert rule
forge alerts rules
# The rules of one workspace, on one page
forge alerts rules --workspace <workspace-id> --all
```

**Output**

```text
$ forge alerts rules
ID                                    NAME          ENABLED  SEVERITY  SUBJECT            REVISION
01a1daed-5291-761e-8a7f-1012aafdddb1  Node offline  true     critical  node.agent.status  1
```


### forge alerts rule

Shows one rule, with `condition`, a plain description of its condition.
Needs `alert.view`.

**Synopsis**

```text
forge alerts rule <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | rule id | — | yes | The rule | `<rule-id>` |

**Examples**

```sh
# One rule
forge alerts rule <rule-id>
# Save a rule as JSON to edit it
forge alerts rule <rule-id> -o json > rule.json
```

**Output**

The rule as JSON, in the shape of the [REST rule body](/docs/reference/rest-api/#alerts)
plus `id`, `workspace_id`, `revision`, `created_by`, `created_at`,
`updated_at` and `condition` (for example `"is offline for 1 min"`).

### forge alerts subjects

Lists every subject a rule can watch, with its values or unit, the scope
kind it applies to and where its evidence comes from, then the health check
subjects seen in the workspace. Needs `alert.view` on the workspace. The
[subjects table](/docs/concepts/alerts/#subjects) explains each one.

**Synopsis**

```text
forge alerts subjects [--workspace <WORKSPACE>]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--workspace <WORKSPACE>` | workspace id | the server's workspace | no | Whose health checks to list (`seen`) | `--workspace <workspace-id>` |

**Examples**

```sh
# Subjects a rule can watch
forge alerts subjects
# Subjects of another workspace
forge alerts subjects --workspace <workspace-id>
```

**Output**

`VALUES` is the unit of a number subject or the states of a state subject.
Rows marked `(seen)` are health checks reported by the workspace's
services (shortened):

```text
$ forge alerts subjects
SUBJECT                     KIND    VALUES                            SCOPE          SOURCE
node.agent.status           state   online|offline|pending            servers        control_plane
node.cpu.percent            number  percent                           servers        agent
node.disk.max_used_percent  number  percent                           servers        agent
node.load.per_cpu           number  ratio                             servers        agent
app.instance.state          state   active|inactive|failed            services   control_plane
package.state               state   running|stopped|not-installed     installations  control_plane
tls.cert.expires_in_days    number  days                              certificates   control_plane
health.readiness.{check}    state   pending|passing|failing|disabled  services   probe
health.readiness.http       state   (seen)                            services   probe
```

With `-o json`, the API answer: `{subjects: [...], seen_health: [...]}`.

### forge alerts templates

Lists the [rule templates](/docs/reference/alert-rule-templates/): ready-made
rules for nodes, services, databases and certificates. Create a rule from
one with `forge alerts create-rule --template <ID>`. Needs `alert.view` on
the workspace.

**Synopsis**

```text
forge alerts templates [--place <PLACE>] [--workspace <WORKSPACE>]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--place <PLACE>` | `node`, `service`, `database`, `certificate` or `any` | all | no | Only the templates for this place | `--place database` |
| `--workspace <WORKSPACE>` | workspace id | the server's workspace | no | The workspace whose `alert.view` is checked | `--workspace <workspace-id>` |

**Examples**

```sh
# Every rule template
forge alerts templates
# Templates for databases
forge alerts templates --place database
```

**Output**

```text
$ forge alerts templates --place database
ID                             PLACE     SEVERITY  SUBJECT                     CONDITION
database-stopped               database  critical  package.state               is stopped for 2 min
database-operation-failed      database  major     package.status              is failed
database-host-disk-filling     database  warning   node.disk.max_used_percent  > 85.0 for 5 min
database-host-memory-pressure  database  major     node.mem.used_percent       > 90.0 for 10 min
```

With `-o json`, each template in full: `id`, `place`, `name`,
`description`, `severity`, `subject`, `scope` and `spec`.

### forge alerts create-rule

Creates a rule, from a JSON document or from a
[template](/docs/reference/alert-rule-templates/). Needs `alert.manage`;
audited as `alert.rule.create`. The document has the fields of the
[rule body](/docs/reference/rest-api/#alerts).

With `--template`, the target flags choose what the rule watches. They must
fit the template's scope: `--node` for node templates, `--service` for service
templates, `--package` for database templates and `--certificate` for
certificate templates. `--package` also fits a database-host template: the
rule then watches the installation's node. Without a target flag the rule
watches every resource of the template's kind in the workspace.

**Synopsis**

```text
forge alerts create-rule <FILE>
forge alerts create-rule --template <ID> [--node <NODE>]... [--service <SERVICE>]... [--package <PACKAGE>]...
    [--certificate <CERT>]... [--check <NAME>] [--severity <SEVERITY>] [--name <NAME>]
    [--workspace <WORKSPACE>] [--disabled]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<file>` | `@path`, `@-` or a path | — | unless `--template` | The rule as JSON: `@rule.json`, `@-` for stdin. Not with `--template` | `@node-offline.json` |
| `--template <ID>` | template id | none | unless `<file>` | Create the rule from this template | `--template node-disk-filling` |
| `--node <NODE>` | node id, hostname or name; repeatable | every node | no | Nodes a node template watches | `--node web-1 --node web-2` |
| `--service <SERVICE>` | service id or name; repeatable | every service | no | Services a service template watches | `--service shop` |
| `--package <PACKAGE>` | installation id or package name; repeatable | every installation | no | Installations a database template watches. A package name must match one installation; use the id when it is installed on several nodes | `--package postgresql` |
| `--certificate <CERT>` | certificate id or name; repeatable | every certificate | no | Uploaded ingress certificates a certificate template watches | `--certificate www` |
| `--check <NAME>` | health check name | none | for `{check}` templates | Fills `{check}` in a service template's subject | `--check http` |
| `--severity <SEVERITY>` | `info`, `warning`, `major` or `critical` | the template's | no | Override the severity | `--severity critical` |
| `--name <NAME>` | text, 1–120 characters | the template's name | no | Override the rule name | `--name "Shop DB stopped"` |
| `--workspace <WORKSPACE>` | workspace id | the server's workspace | no | Create the rule in this workspace | `--workspace <workspace-id>` |
| `--disabled` | flag | off | no | Create the rule disabled | `--disabled` |

A target flag that does not fit the template, or `{check}` without
`--check`, exits with `2` before anything is sent.

**Examples**

```sh
# A node offline for a minute opens a critical alarm
forge alerts create-rule @node-offline.json

# The disk-filling template on two nodes
forge alerts create-rule --template node-disk-filling --node web-1 --node web-2

# Alert when the PostgreSQL service stops
forge alerts create-rule --template database-stopped --package postgresql

# Alert when the shop's "http" liveness check fails
forge alerts create-rule --template service-liveness-failing --service shop --check http
```

See [Create alerts from templates](/docs/guides/alert-templates/) and
[Create an alert rule](/docs/guides/alerts/#create-the-rule).

**Output**

The new rule as JSON, with `revision` `1` (shortened):

```text
$ forge alerts create-rule --template database-stopped --package postgresql
{
  "enabled": true,
  "id": "<rule-id>",
  "name": "Database stopped",
  "revision": 1,
  "scope": { "ids": ["<installation-id>"], "kind": "installations" },
  "severity": "critical",
  "spec": {
    "mode": { "for_ms": 120000, "mode": "sustained" },
    "recovery": { "condition": null, "hold_ms": 0 },
    "trigger": { "kind": "state_in", "states": ["stopped"] }
  },
  "subject": "package.state",
  "workspace_id": "<workspace-id>"
}
```


### forge alerts update-rule

Replaces a rule. A changed subject, scope, condition or `enabled` starts a
new revision, and firing alarms of the old revision end. Needs
`alert.manage`; audited as `alert.rule.update`.

**Synopsis**

```text
forge alerts update-rule <ID> <FILE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | rule id | — | yes | The rule | `<rule-id>` |
| `<file>` | `@path`, `@-` or a path | — | yes | The new rule, same fields as create | `@node-offline.json` |

**Examples**

```sh
# Replace a rule from a file
forge alerts update-rule <rule-id> @rule.json
# Replace it from standard input
cat rule.json | forge alerts update-rule <rule-id> @-
```

**Output**

The rule after the change, in the same shape as create.

### forge alerts delete-rule

Deletes a rule. Its alarms stay as history; firing ones end. Needs
`alert.manage`; audited as `alert.rule.delete`. Asks for confirmation on a
terminal.

**Synopsis**

```text
forge alerts delete-rule <ID> [--yes]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | rule id | — | yes | The rule | `<rule-id>` |
| `--yes` | flag | off | no | Do not ask for confirmation | `--yes` |

**Examples**

```sh
# Delete a rule after you confirm
forge alerts delete-rule <rule-id>
# Delete it without asking (scripts)
forge alerts delete-rule <rule-id> --yes
```

**Output**

```text
deleted  <rule-id>
```

## Notification channels

A channel is where a message goes: a `webhook` receives Forge's
[JSON payload](/docs/reference/notification-payload/), a `slack` channel
receives a text message that Slack incoming webhooks and compatible
receivers understand. Reading channels needs `alert.view`. Creating,
changing, deleting and testing one needs `notification.manage` (workspace
administrator or platform administrator), which is MFA-guarded where the
server requires MFA.

:::caution
A channel URL usually carries a token. The CLI reads it from a file or from
stdin and refuses a URL on the command line, where it would stay in the
shell history and the process list. The server stores it sealed and never
returns it: every answer shows `target`, the URL's scheme, host and port.
:::

### forge alerts channels list

Lists the workspace's channels by name. Needs `alert.view` on the
workspace.

**Synopsis**

```text
forge alerts channels list [--workspace <WORKSPACE>] [--limit <N>] [--page <N>] [--all]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--workspace <WORKSPACE>` | workspace id | this deployment's workspace | no | Only this workspace's channels | `--workspace <workspace-id>` |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

**Examples**

```sh
# Every notification channel
forge alerts channels list
# Every channel on one page
forge alerts channels list --all
```

**Output**

```text
$ forge alerts channels list
ID                                    NAME         KIND     ENABLED  TARGET                     SECRET ITEM
01a1e2d0-bcbb-7005-9872-8b2c7d39e099  Ops webhook  webhook  yes      https://hooks.example.com  -
01a1e2d0-bce2-710e-9d33-2307f06e2127  Team chat    slack    yes      https://chat.example.com   -
showing 1–2 of 2
```

`SECRET ITEM` is the id of the vault item that signs the channel's
messages, or `-`. With `-o json` each row has the
[REST channel fields](/docs/reference/rest-api/#notifications).

### forge alerts channels show

Shows one channel. Needs `alert.view`. `forge alerts channels get` is the
same command.

**Synopsis**

```text
forge alerts channels show <CHANNEL>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<channel>` | channel id or name | — | yes | The channel. A name must match exactly one channel (case does not matter) | `"Ops webhook"` |

**Examples**

```sh
# One channel, by name
forge alerts channels show "Ops webhook"
# One channel, by id
forge alerts channels show <channel-id>
```

**Output**

```text
$ forge alerts channels show "Ops webhook"
created_at    2026-11-16 10:24:53
created_by:
  id    01a1e2d0-b06b-74d5-b34a-b7450e5f7fa6
  type  user
enabled       yes
id            01a1e2d0-bcbb-7005-9872-8b2c7d39e099
kind          webhook
name          Ops webhook
target        https://hooks.example.com
updated_at    2026-11-16 10:24:53
workspace_id  01a1e2d0-b216-7078-988a-a2bbd9e3357f
```

A signed channel also has `secret_item`. Times are local; with `-o json`
they are epoch milliseconds.

### forge alerts channels create

Creates a channel. Needs `notification.manage`; audited as
`notification.channel.create` (the audit entry holds the target host, never
the URL). A workspace holds at most 50 channels.

**Synopsis**

```text
forge alerts channels create --name <NAME> --kind webhook|slack --url <@FILE> [--secret-item <ITEM>] [--disabled]
forge alerts channels create --name <NAME> --kind email --smtp-host <HOST> --from <ADDRESS> --to <ADDRESS>...
    [--smtp-port <PORT>] [--tls <TLS>] [--smtp-user <USER> --smtp-password <@FILE>] [--subject-prefix <TEXT>] [--disabled]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--name <NAME>` | string, 1 to 80 characters | — | yes | The channel's name | `--name "Ops webhook"` |
| `--kind <KIND>` | `webhook`, `slack` or `email` | — | yes | What the receiver understands | `--kind webhook` |
| `--url <@FILE>` | `@<path>` or `@-` | — | `webhook`, `slack`: yes | Where the URL is read from: a file, or stdin with `@-`. One line; a trailing line break is dropped. A URL typed after `--url` is refused (exit `2`, code `secret_on_command_line`). The URL is `https://host[:port]/path`, at most 2000 bytes | `--url @ops-url.txt` |
| `--secret-item <ITEM>` | vault item id or name | none | no | Signs each webhook body with this item's password (HMAC-SHA256). `webhook` only. The server also checks that you may use the item: platform administrator, `secret.read`, and not with an API key | `--secret-item webhook-signing-key` |
| `--no-secret-item` | flag | off | no | No signing. Has no effect on a new channel; see [update](#forge-alerts-channels-update) | `--no-secret-item` |
| `--enabled` | flag | on | no | Creates the channel turned on, which is the default. Cannot be combined with `--disabled` | `--enabled` |
| `--disabled` | flag | off | no | Creates the channel turned off: nothing is sent to it | `--disabled` |
| `--smtp-host <HOST>` | host name or IP address | — | `email`: yes | The SMTP server. The same address rules as a webhook URL apply | `--smtp-host smtp.example.com` |
| `--smtp-port <PORT>` | integer, 1 to 65535 | `587` (`starttls`), `465` (`tls`), `25` (`none`) | no | The SMTP port | `--smtp-port 2525` |
| `--tls <TLS>` | `starttls`, `tls` or `none` | `starttls` | no | `starttls`: STARTTLS is required, never optional. `tls`: TLS from the first byte (SMTPS). `none`: unencrypted, only when the server sets `[alerts] webhook_allow_http`, and never with `--smtp-user` | `--tls tls` |
| `--smtp-user <USER>` | string, at most 256 bytes | none | with a password | SMTP user name. Needs `--smtp-password` | `--smtp-user forge@example.com` |
| `--no-smtp-user` | flag | off | no | Send without authentication. Has no effect on a new channel; see [update](#forge-alerts-channels-update) | `--no-smtp-user` |
| `--smtp-password <@FILE>` | `@<path>` or `@-` | — | with `--smtp-user` | Where the SMTP password is read from: a file, or stdin with `@-`. One line; a trailing line break is dropped, spaces stay. A password typed after the flag is refused (exit `2`, code `secret_on_command_line`). Stored encrypted and never shown | `--smtp-password @smtp-password.txt` |
| `--from <ADDRESS>` | email address | — | `email`: yes | The sender: `forge@example.com` or `"Forge <forge@example.com>"` | `--from "Forge <forge@example.com>"` |
| `--to <ADDRESS>` | email address, repeatable | — | `email`: at least one | A recipient. Repeat for up to 20; duplicates are dropped | `--to ops@example.com --to oncall@example.com` |
| `--subject-prefix <TEXT>` | string, at most 40 characters | none | no | Put before every subject | `--subject-prefix "[prod]"` |

The SMTP flags are for `--kind email` only; an email channel takes no
`--url` and no `--secret-item` (exit `2`). The server refuses an address that
does not parse, more than 20 recipients, and an SMTP host that breaks the
same address rules as a webhook URL (exit `2`).

The server refuses a URL (exit `2`) that is not `https` unless
`[alerts] webhook_allow_http` is set, that has a user name, password or
fragment, or whose host is a loopback, private or metadata address that the
[`[alerts]` settings](/docs/reference/configuration/#alerts) do not allow.

**Examples**

```sh
# The URL is the only line of a file that only you can read
forge alerts channels create --name "Ops webhook" --kind webhook --url @ops-url.txt

# The URL comes from a secret manager through a pipe
printf '%s' "$CHAT_WEBHOOK_URL" | forge alerts channels create --name "Team chat" --kind slack --url @-

# A signed webhook: the receiver verifies X-Forge-Signature
forge alerts channels create --name "Pager" --kind webhook --url @pager-url.txt --secret-item webhook-signing-key

# Email through an SMTP relay with STARTTLS; the password comes from a file
forge alerts channels create --name "Ops mail" --kind email \
  --smtp-host smtp.example.com --smtp-user forge@example.com --smtp-password @smtp-password.txt \
  --from "Forge <forge@example.com>" --to ops@example.com --to oncall@example.com --subject-prefix "[prod]"
```

**Output**

The new channel, as in [`forge alerts channels show`](#forge-alerts-channels-show):

```text
$ forge alerts channels create --name "Ops webhook" --kind webhook --url @ops-url.txt
created_at    2026-11-16 10:24:53
created_by:
  id    01a1e2d0-b06b-74d5-b34a-b7450e5f7fa6
  type  user
enabled       yes
id            01a1e2d0-bcbb-7005-9872-8b2c7d39e099
kind          webhook
name          Ops webhook
target        https://hooks.example.com
updated_at    2026-11-16 10:24:53
workspace_id  01a1e2d0-b216-7078-988a-a2bbd9e3357f

$ forge alerts channels create --name Refused --kind slack --url https://hooks.example.com/x
error: --url: a channel URL carries a token and is never taken from the command line; pass `--url @<file>`, or pipe it on stdin and pass `--url @-` [secret_on_command_line]
```

An email channel shows its SMTP settings, never the password:

```text
$ forge alerts channels create --name "Ops mail" --kind email --smtp-host smtp.example.com \
    --smtp-user forge@example.com --smtp-password @smtp-password.txt \
    --from "Forge <forge@example.com>" --to ops@example.com --subject-prefix "[prod]"
created_at    2026-11-16 10:31:07
created_by:
  id    01a1e2d0-b06b-74d5-b34a-b7450e5f7fa6
  type  user
email:
  from            Forge <forge@example.com>
  password_set    yes
  smtp_host       smtp.example.com
  smtp_port       587
  subject_prefix  [prod]
  tls             starttls
  to:
    - ops@example.com
  username        forge@example.com
enabled       yes
id            01a1e2d6-2f3c-7a41-a0d4-5f1c9e8b7a62
kind          email
name          Ops mail
target        smtp://smtp.example.com:587
updated_at    2026-11-16 10:31:07
workspace_id  01a1e2d0-b216-7078-988a-a2bbd9e3357f

$ forge alerts channels create --name Refused --kind email --smtp-host smtp.example.com \
    --smtp-user forge --smtp-password hunter2 --from forge@example.com --to ops@example.com
error: --smtp-password: an SMTP password is never taken from the command line; pass `--smtp-password @<file>`, or pipe it on stdin and pass `--smtp-password @-` [secret_on_command_line]
```

### forge alerts channels update

Changes a channel. The command reads the stored channel, changes the fields
that the flags name and keeps the others, then sends the whole channel.
Without `--url` the stored URL stays. Pass at least one flag. Needs
`notification.manage`; audited as `notification.channel.update`. Asks for
confirmation on a terminal.

**Synopsis**

```text
forge alerts channels update <CHANNEL> [--name <NAME>] [--kind <KIND>] [--url <@FILE>]
    [--secret-item <ITEM> | --no-secret-item] [--enabled | --disabled]
    [--smtp-host <HOST>] [--smtp-port <PORT>] [--tls <TLS>] [--smtp-user <USER> | --no-smtp-user]
    [--smtp-password <@FILE>] [--from <ADDRESS>] [--to <ADDRESS>...] [--subject-prefix <TEXT>] [--yes]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<channel>` | channel id or name | — | yes | The channel | `"Team chat"` |
| `--name <NAME>` | string, 1 to 80 characters | the stored name | no | New name | `--name Chat` |
| `--kind <KIND>` | `webhook`, `slack` or `email` | the stored kind | no | New kind. A signed channel becomes `slack` or `email` only together with `--no-secret-item`. An email channel becomes `webhook` or `slack` only with a new `--url`; a channel becomes `email` with `--smtp-host`, `--from` and `--to` | `--kind slack` |
| `--url <@FILE>` | `@<path>` or `@-` | the stored URL | no | Replaces the URL, read from a file or from stdin as in [create](#forge-alerts-channels-create). With `@-`, pass `--yes` too: stdin cannot also answer the confirmation | `--url @new-url.txt` |
| `--secret-item <ITEM>` | vault item id or name | the stored item | no | Signs with this vault item from now on. Cannot be combined with `--no-secret-item` | `--secret-item webhook-signing-key` |
| `--no-secret-item` | flag | off | no | Stops signing: removes the stored secret item | `--no-secret-item` |
| `--enabled` | flag | the stored state | no | Turns the channel on. Cannot be combined with `--disabled` | `--enabled` |
| `--disabled` | flag | the stored state | no | Turns the channel off. Its pending deliveries are cancelled | `--disabled` |
| `--smtp-host <HOST>` | host name or IP address | the stored host | no | New SMTP server | `--smtp-host smtp2.example.com` |
| `--smtp-port <PORT>` | integer, 1 to 65535 | the stored port; with a new `--tls`, that mode's usual port | no | New SMTP port | `--smtp-port 465` |
| `--tls <TLS>` | `starttls`, `tls` or `none` | the stored mode | no | New connection protection. Without `--smtp-port`, a new mode takes its usual port | `--tls tls` |
| `--smtp-user <USER>` | string, at most 256 bytes | the stored user | no | New SMTP user name. A channel that had no user name needs `--smtp-password` too | `--smtp-user alerts@example.com` |
| `--no-smtp-user` | flag | off | no | Stops authenticating: removes the stored user name and password. Cannot be combined with `--smtp-user` or `--smtp-password` | `--no-smtp-user` |
| `--smtp-password <@FILE>` | `@<path>` or `@-` | the stored password | when `--smtp-host`, `--smtp-port`, `--tls` or `--smtp-user` changes | Replaces the SMTP password, read as in [create](#forge-alerts-channels-create). With `@-`, pass `--yes` too. Only one of `--url` and `--smtp-password` can read stdin | `--smtp-password @new-password.txt` |
| `--from <ADDRESS>` | email address | the stored sender | no | New sender | `--from alerts@example.com` |
| `--to <ADDRESS>` | email address, repeatable | the stored recipients | no | Given at all, the flags replace the whole recipient list (1 to 20) | `--to ops@example.com` |
| `--subject-prefix <TEXT>` | string, at most 40 characters | the stored prefix | no | New subject prefix; `""` removes it | `--subject-prefix ""` |
| `--yes` | flag | off | no | Do not ask for confirmation (required with `--non-interactive`) | `--yes` |

**Examples**

```sh
# Rename a channel and turn it off
forge alerts channels update "Team chat" --name Chat --disabled --yes

# Rotate the URL after its token leaked
forge alerts channels update "Ops webhook" --url @new-url.txt

# Rotate an email channel's SMTP password; the other settings stay
forge alerts channels update "Ops mail" --smtp-password @new-password.txt --yes

# Replace the recipients of an email channel
forge alerts channels update "Ops mail" --to ops@example.com --to sre@example.com --yes
```

**Output**

The channel after the change:

```text
$ forge alerts channels update "Team chat" --name Chat --disabled --yes
created_at    2026-11-16 10:24:53
created_by:
  id    01a1e2d0-b06b-74d5-b34a-b7450e5f7fa6
  type  user
enabled       no
id            01a1e2d0-bce2-710e-9d33-2307f06e2127
kind          slack
name          Chat
target        https://chat.example.com
updated_at    2026-11-16 10:24:53
workspace_id  01a1e2d0-b216-7078-988a-a2bbd9e3357f
```

### forge alerts channels delete

Deletes a channel. Its delivery history stays. A channel that a policy
still uses is refused (exit `7`): take it out of the policy first. Needs
`notification.manage`; audited as `notification.channel.delete`. Asks for
confirmation on a terminal.

**Synopsis**

```text
forge alerts channels delete <CHANNEL> [--yes]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<channel>` | channel id or name | — | yes | The channel | `Chat` |
| `--yes` | flag | off | no | Do not ask for confirmation (required with `--non-interactive`) | `--yes` |

**Examples**

```sh
# Delete a channel after you confirm
forge alerts channels delete "Ops webhook"
# Delete it without asking (scripts)
forge alerts channels delete "Ops webhook" --yes
```

**Output**

```text
$ forge alerts channels delete Chat --yes
deleted  01a1e2d0-bce2-710e-9d33-2307f06e2127
name     Chat

$ forge alerts channels delete "Ops webhook" --yes
error: the channel is used by Critical to ops: remove it from the policy first [conflict]
```

### forge alerts channels test

Sends one test message to the channel now and prints the delivery with its
attempt. The command exits `0` only when the receiver accepted the message
(status `sent`); any other status exits `10` with the code
`delivery_failed`. A test is tried once and never retried. Needs
`notification.manage`; audited as `notification.channel.test` whether the
receiver accepted the message or not.

**Synopsis**

```text
forge alerts channels test <CHANNEL>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<channel>` | channel id or name | — | yes | The channel. A channel that is turned off is refused (exit `7`) | `"Ops webhook"` |

**Examples**

```sh
# Send a test notification to a channel
forge alerts channels test "Ops webhook"
```

**Output**

```text
$ forge alerts channels test "Ops webhook"
attempt_count     1
channel_id        01a1e2d0-bcbb-7005-9872-8b2c7d39e099
channel_name      Ops webhook
created_at        2026-11-16 10:24:53
event             test
finished_at       2026-11-16 10:24:53
id                01a1e2d0-bd68-7074-bf9e-41a60ce14589
last_http_status  204
next_attempt_at   2026-11-16 10:25:53
occurrence        0
status            sent
workspace_id      01a1e2d0-b216-7078-988a-a2bbd9e3357f
attempts:
  NUMBER  AT                   OK   HTTP  ERROR  DURATION (MS)
  1       2026-11-16 10:24:53  yes  204   -      2
```

A receiver that answers `500` (exit `10`):

```text
$ forge alerts channels test "Team chat"
attempt_count     1
channel_id        01a1e2d0-bce2-710e-9d33-2307f06e2127
channel_name      Team chat
created_at        2026-11-16 10:24:53
event             test
finished_at       2026-11-16 10:24:53
id                01a1e2d0-bd8f-734a-8911-6f4f8256e3fa
last_error        http_status
last_http_status  500
next_attempt_at   2026-11-16 10:25:53
occurrence        0
status            failed
workspace_id      01a1e2d0-b216-7078-988a-a2bbd9e3357f
attempts:
  NUMBER  AT                   OK  HTTP  ERROR        DURATION (MS)
  1       2026-11-16 10:24:53  no  500   http_status  1
error: the test message to channel Team chat was not delivered (status failed, HTTP 500, error http_status); see `forge alerts deliveries show 01a1e2d0-bd8f-734a-8911-6f4f8256e3fa` [delivery_failed]
```

`ERROR` is a class, never the receiver's text: `timeout`, `connect`, `tls`,
`blocked_target`, `http_status`, `smtp_reply`, `secret` or `config`. For an
email channel the message says `SMTP <code>` instead of `HTTP <code>`. With
`-o json` a delivered test prints the delivery and a failed one prints only the error
document, which names the delivery to look up:

```text
$ forge alerts channels test "Team chat" -o json
{"error":{"class":"job_failed","code":"delivery_failed","exit_code":10,"message":"the test message to channel Team chat was not delivered (status failed, HTTP 500, error http_status); see `forge alerts deliveries show 01a1e2d0-bdb7-76e5-827f-4d66628538da`","request_id":null,"retryable":false},"schema":"forge.cli/v1"}

$ forge alerts channels test Chat
error: the channel is turned off [conflict]
```

## Notification policies

A policy says which alarms notify which channels: alarms of a minimum
severity, optionally only of some rules or on some resources, when they
fire, when they resolve, or both. Reading policies needs `alert.view`;
creating, changing and deleting one needs `alert.manage` (operator,
workspace administrator or platform administrator). A channel that two
matching policies name gets one message per alarm event.

### forge alerts policies list

Lists the workspace's policies by name. Needs `alert.view` on the
workspace.

**Synopsis**

```text
forge alerts policies list [--workspace <WORKSPACE>] [--limit <N>] [--page <N>] [--all]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--workspace <WORKSPACE>` | workspace id | this deployment's workspace | no | Only this workspace's policies | `--workspace <workspace-id>` |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

**Examples**

```sh
# Every notification policy
forge alerts policies list
# Every policy on one page
forge alerts policies list --all
```

**Output**

```text
$ forge alerts policies list
ID                                    NAME             ENABLED  MIN SEVERITY  EVENTS           CHANNELS     REPEAT (S)
01a1e2d0-be2b-717b-9ea8-1aa2b40add10  Critical to ops  yes      critical      fired, resolved  Ops webhook  3600
showing 1–1 of 1
```

`CHANNELS` shows channel names and `REPEAT (S)` the reminder interval in
seconds (`-` for no reminders). With `-o json` each row has the
[REST policy fields](/docs/reference/rest-api/#notifications), with `channels`
as ids, plus `channel_names`.

### forge alerts policies show

Shows one policy. Needs `alert.view`. `forge alerts policies get` is the
same command.

**Synopsis**

```text
forge alerts policies show <POLICY>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<policy>` | policy id or name | — | yes | The policy. A name must match exactly one policy (case does not matter) | `"Critical to ops"` |

**Examples**

```sh
# One policy, by name
forge alerts policies show "Critical to ops"
# One policy, by id
forge alerts policies show <policy-id>
```

**Output**

```text
$ forge alerts policies show "Critical to ops"
channels              01a1e2d0-bcbb-7005-9872-8b2c7d39e099
created_at            2026-11-16 10:24:53
created_by:
  id    01a1e2d0-b06b-74d5-b34a-b7450e5f7fa6
  type  user
enabled               yes
events                fired, resolved
id                    01a1e2d0-be2b-717b-9ea8-1aa2b40add10
min_severity          critical
name                  Critical to ops
repeat_interval_secs  3600
scope:
  kind  servers
updated_at            2026-11-16 10:24:53
workspace_id          01a1e2d0-b216-7078-988a-a2bbd9e3357f
```

The view leaves out empty fields: `rules` when the policy covers every
rule, the scope's `ids` when it covers all of that kind, and `scope` and
`repeat_interval_secs` when they are not set.

### forge alerts policies create

Creates a policy. Needs `alert.manage`; audited as
`notification.policy.create`. A workspace holds at most 100 policies.

**Synopsis**

```text
forge alerts policies create --name <NAME> --min-severity <SEVERITY> --event <EVENT>... --channel <CHANNEL>...
    [--rule <RULE>...] [--scope <SCOPE> [--scope-id <ID>...]] [--repeat <DURATION>]
    [--email-channel <CHANNEL> (--email-user <USER>... | --email-role <ROLE>...)] [--disabled]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--name <NAME>` | string, 1 to 80 characters | — | yes | The policy's name | `--name "Critical to ops"` |
| `--min-severity <SEVERITY>` | `info`, `warning`, `major` or `critical` | — | yes | Notifies for alarms of this severity and worse | `--min-severity critical` |
| `--event <EVENT>` | `fired` or `resolved`; repeatable | — | yes, at least one | The alarm events to notify for. `resolved` also covers an alarm whose tracking ended without recovery | `--event fired --event resolved` |
| `--channel <CHANNEL>` | channel id or name; repeatable, up to 10 | — | yes, at least one, unless the policy emails people (`--email-user` or `--email-role`) | The channels to notify | `--channel "Ops webhook"` |
| `--rule <RULE>` | rule id; repeatable, up to 100 | every rule | no | Only alarms of these rules. Cannot be combined with `--all-rules` | `--rule <rule-id>` |
| `--all-rules` | flag | on | no | Alarms of every rule, which is the default for a new policy | `--all-rules` |
| `--scope <SCOPE>` | `servers`, `services` or `all` | `all` | no | Only alarms on nodes, or only alarms on services. `all` sets no limit | `--scope servers` |
| `--scope-id <ID>` | node id, host name or name with `--scope servers`; service id with `--scope services`; repeatable, up to 500 | all of that kind | no | Only these nodes or services. Needs `--scope`; not allowed with `--scope all` | `--scope-id web-01` |
| `--repeat <DURATION>` | duration (`30m`, `4h`, `1d`, `1h30m`; units `m`, `h`, `d`, `w`), whole seconds, or `never` | `never` | no | Sends a reminder this often while the alarm fires and nobody has acknowledged it. From `5m` (`300`) to `7d` (`604800`). Needs `--event fired`. An alarm sends at most 50 reminders to a channel | `--repeat 1h` |
| `--email-user <USER>` | user id, username or email; repeatable, up to 100 | none | no | Emails this person when they may see the alarm (`alert.view` on its resource) and have an email address. Needs `--email-channel` | `--email-user ana@example.com` |
| `--email-role <ROLE>` | `platform_admin`, `workspace_admin`, `operator`, `developer`, `dba`, `auditor` or `customer`; repeatable, up to 20 | none | no | Emails everyone who holds this role on the alarm's resource or on any resource above it (environment, cluster, workspace, platform). Needs `--email-channel` | `--email-role operator` |
| `--email-channel <CHANNEL>` | email channel id or name | — | with `--email-user` or `--email-role` | The email channel (SMTP server) that sends to the people. Its own `--to` addresses are not added | `--email-channel "Ops mail"` |
| `--no-email-recipients` | flag | off | no | Emails nobody by user or role. On create it is the same as leaving the email flags out. Cannot be combined with the other email flags | `--no-email-recipients` |
| `--enabled` | flag | on | no | Creates the policy turned on, which is the default. Cannot be combined with `--disabled` | `--enabled` |
| `--disabled` | flag | off | no | Creates the policy turned off: it notifies nobody | `--disabled` |

**Examples**

```sh
# Critical alarms email the operators of the affected resource and one named person
forge alerts policies create --name "Page operators" --min-severity critical --event fired \
  --email-channel "Ops mail" --email-role operator --email-user ana@example.com

# Critical alarms on any node go to the ops webhook, with a reminder every hour
forge alerts policies create --name "Critical to ops" --min-severity critical \
  --event fired --event resolved --channel "Ops webhook" --scope servers --repeat 1h

# Warnings and worse of one rule on two nodes go to the chat when they fire
forge alerts policies create --name "Disk warnings" --min-severity warning --event fired \
  --channel "Team chat" --rule <rule-id> --scope servers --scope-id web-01 --scope-id web-02
```

**Output**

The new policy, as in [`forge alerts policies show`](#forge-alerts-policies-show):

```text
$ forge alerts policies create --name "Critical to ops" --min-severity critical --event fired --event resolved --channel "Ops webhook" --scope servers --repeat 1h
channels              01a1e2d0-bcbb-7005-9872-8b2c7d39e099
created_at            2026-11-16 10:24:53
created_by:
  id    01a1e2d0-b06b-74d5-b34a-b7450e5f7fa6
  type  user
enabled               yes
events                fired, resolved
id                    01a1e2d0-be2b-717b-9ea8-1aa2b40add10
min_severity          critical
name                  Critical to ops
repeat_interval_secs  3600
scope:
  kind  servers
updated_at            2026-11-16 10:24:53
workspace_id          01a1e2d0-b216-7078-988a-a2bbd9e3357f
```

### forge alerts policies update

Changes a policy. The command reads the stored policy, changes the fields
that the flags name and keeps the others, then sends the whole policy. A
repeatable flag (`--event`, `--channel`, `--rule`, `--scope-id`, `--email-user`, `--email-role`) replaces
its whole list: name every value the list should hold. Pass at least one
flag. Needs `alert.manage`; audited as `notification.policy.update`. Asks
for confirmation on a terminal.

**Synopsis**

```text
forge alerts policies update <POLICY> [--name <NAME>] [--min-severity <SEVERITY>] [--event <EVENT>...]
    [--channel <CHANNEL>...] [--rule <RULE>... | --all-rules] [--scope <SCOPE> [--scope-id <ID>...]]
    [--repeat <DURATION>] [--email-channel <CHANNEL>] [--email-user <USER>...] [--email-role <ROLE>...]
    [--no-email-recipients] [--enabled | --disabled] [--yes]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<policy>` | policy id or name | — | yes | The policy | `"Critical to ops"` |
| `--name <NAME>` | string, 1 to 80 characters | the stored name | no | New name | `--name "Major to ops"` |
| `--min-severity <SEVERITY>` | `info`, `warning`, `major` or `critical` | the stored severity | no | New minimum severity | `--min-severity major` |
| `--event <EVENT>` | `fired` or `resolved`; repeatable | the stored events | no | Replaces the event list | `--event fired` |
| `--channel <CHANNEL>` | channel id or name; repeatable, 1 to 10 | the stored channels | no | Replaces the channel list | `--channel "Ops webhook" --channel Chat` |
| `--rule <RULE>` | rule id; repeatable, up to 100 | the stored rules | no | Replaces the rule list. Cannot be combined with `--all-rules` | `--rule <rule-id>` |
| `--all-rules` | flag | off | no | Removes the stored rule list: alarms of every rule | `--all-rules` |
| `--scope <SCOPE>` | `servers`, `services` or `all` | the stored scope | no | Replaces the scope, together with `--scope-id`. `all` removes it | `--scope all` |
| `--scope-id <ID>` | node id, host name or name; service id; repeatable, up to 500 | all of that kind | no | The nodes or services of `--scope`. Needs `--scope` | `--scope servers --scope-id web-01` |
| `--repeat <DURATION>` | duration, whole seconds, or `never` | the stored interval | no | New reminder interval, as in [create](#forge-alerts-policies-create). `never` turns reminders off | `--repeat never` |
| `--email-user <USER>` | user id, username or email; repeatable, up to 100 | the stored users | no | Replaces the list of people emailed by name | `--email-user ana --email-user bo@example.com` |
| `--email-role <ROLE>` | role name, as in [create](#forge-alerts-policies-create); repeatable, up to 20 | the stored roles | no | Replaces the list of roles whose holders are emailed | `--email-role operator` |
| `--email-channel <CHANNEL>` | email channel id or name | the stored email channel | no | The email channel that sends to the people | `--email-channel "Ops mail"` |
| `--no-email-recipients` | flag | off | no | Removes the stored users, roles and email channel. The policy then needs at least one channel. Cannot be combined with the other email flags | `--no-email-recipients` |
| `--enabled` | flag | the stored state | no | Turns the policy on. Cannot be combined with `--disabled` | `--enabled` |
| `--disabled` | flag | the stored state | no | Turns the policy off | `--disabled` |
| `--yes` | flag | off | no | Do not ask for confirmation (required with `--non-interactive`) | `--yes` |

**Examples**

```sh
# Lower the threshold and stop the reminders; channels, events and scope stay
forge alerts policies update "Critical to ops" --min-severity major --repeat never --yes

# Notify a second channel: name both, the flags replace the list
forge alerts policies update "Critical to ops" --channel "Ops webhook" --channel Chat

# Also email the operators of the affected resource
forge alerts policies update "Critical to ops" --email-channel "Ops mail" --email-role operator --yes

# Stop emailing people; the channels stay
forge alerts policies update "Critical to ops" --no-email-recipients --yes
```

**Output**

The policy after the change:

```text
$ forge alerts policies update "Critical to ops" --min-severity major --repeat never --yes
channels      01a1e2d0-bcbb-7005-9872-8b2c7d39e099
created_at    2026-11-16 10:24:53
created_by:
  id    01a1e2d0-b06b-74d5-b34a-b7450e5f7fa6
  type  user
enabled       yes
events        fired, resolved
id            01a1e2d0-be2b-717b-9ea8-1aa2b40add10
min_severity  major
name          Critical to ops
scope:
  kind  servers
updated_at    2026-11-16 10:24:53
workspace_id  01a1e2d0-b216-7078-988a-a2bbd9e3357f

$ forge alerts policies update "Critical to ops" --event resolved --repeat 30m --yes
error: reminders (--repeat) need --event fired: pass it, or turn reminders off with --repeat never [invalid_input]
```

### forge alerts policies delete

Deletes a policy. Deliveries that are already planned are still sent. Needs
`alert.manage`; audited as `notification.policy.delete`. Asks for
confirmation on a terminal.

**Synopsis**

```text
forge alerts policies delete <POLICY> [--yes]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<policy>` | policy id or name | — | yes | The policy | `"Critical to ops"` |
| `--yes` | flag | off | no | Do not ask for confirmation (required with `--non-interactive`) | `--yes` |

**Examples**

```sh
# Delete a policy after you confirm
forge alerts policies delete "Critical to ops"
# Delete it without asking (scripts)
forge alerts policies delete "Critical to ops" --yes
```

**Output**

```text
$ forge alerts policies delete "Critical to ops" --yes
deleted  01a1e2d0-be2b-717b-9ea8-1aa2b40add10
name     Critical to ops
```

## Notification deliveries

A delivery is one message: an event of one alarm for one channel of one
policy, or a test, with every attempt to send it. The server keeps finished
deliveries for 30 days. See
[delivery statuses](/docs/reference/notification-payload/#delivery-statuses) for
what each status means.

### forge alerts deliveries list

Lists deliveries, newest first. Needs `alert.view` on the workspace.
Deliveries of alarms on resources that you cannot view are left out.

**Synopsis**

```text
forge alerts deliveries list [--alarm <ALARM>] [--channel <CHANNEL>] [--status <STATUS>]
    [--workspace <WORKSPACE>] [--limit <N>] [--page <N>] [--all]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--alarm <ALARM>` | alarm id | — | no | Only this alarm's deliveries | `--alarm <alarm-id>` |
| `--channel <CHANNEL>` | channel id or name | — | no | Only deliveries to this channel | `--channel "Ops webhook"` |
| `--status <STATUS>` | `pending`, `sending`, `sent`, `failed`, `cancelled` or `suppressed` | — | no | Only deliveries in this status | `--status failed` |
| `--workspace <WORKSPACE>` | workspace id | this deployment's workspace | no | Only this workspace's deliveries | `--workspace <workspace-id>` |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row the server returns | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

The server returns at most the 500 newest deliveries that match. Narrow the
list with `--alarm`, `--channel` or `--status` to reach older ones.

**Examples**

```sh
# Recent deliveries
forge alerts deliveries list
# Deliveries that failed
forge alerts deliveries list --status failed
# Deliveries of one alarm to one channel
forge alerts deliveries list --alarm <alarm-id> --channel "Ops webhook"
```

**Output**

```text
$ forge alerts deliveries list
ID                                    STATUS  EVENT  CHANNEL      ALARM  ATTEMPTS  HTTP  ERROR        CREATED
01a1e2d0-bdb7-76e5-827f-4d66628538da  failed  test   Team chat           1         500   http_status  2026-11-16 10:24:53
01a1e2d0-bd8f-734a-8911-6f4f8256e3fa  failed  test   Team chat           1         500   http_status  2026-11-16 10:24:53
01a1e2d0-bd68-7074-bf9e-41a60ce14589  sent    test   Ops webhook         1         204   -            2026-11-16 10:24:53
showing 1–3 of 3

$ forge alerts deliveries list --status failed --channel Chat
ID                                    STATUS  EVENT  CHANNEL    ALARM  ATTEMPTS  HTTP  ERROR        CREATED
01a1e2d0-bdb7-76e5-827f-4d66628538da  failed  test   Team chat         1         500   http_status  2026-11-16 10:24:53
01a1e2d0-bd8f-734a-8911-6f4f8256e3fa  failed  test   Team chat         1         500   http_status  2026-11-16 10:24:53
showing 1–2 of 2
```

`EVENT` is `fired`, `resolved`, `ended`, `reminder` or `test`. `CHANNEL` is
the channel's name when the delivery was planned, so a channel renamed
since then (here from `Team chat` to `Chat`) keeps its old name in the
history. `ALARM` is the alarm's title, empty for a test. `HTTP` and `ERROR`
describe the last attempt; for an email channel `HTTP` holds the SMTP reply
code (`250`, `451`, `550`). With `-o json` each row has the
[REST delivery fields](/docs/reference/rest-api/#notifications).

### forge alerts deliveries show

Shows one delivery with its attempts. Needs `alert.view`. A delivery of an
alarm on a resource that you cannot view answers as not found (exit `6`).
`forge alerts deliveries get` is the same command.

**Synopsis**

```text
forge alerts deliveries show <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | delivery id | — | yes | The delivery | `<delivery-id>` |

**Examples**

```sh
# One delivery with its attempts
forge alerts deliveries show <delivery-id>
```

**Output**

```text
$ forge alerts deliveries show 01a1e2d0-bdb7-76e5-827f-4d66628538da
attempt_count     1
channel_id        01a1e2d0-bce2-710e-9d33-2307f06e2127
channel_name      Team chat
created_at        2026-11-16 10:24:53
event             test
finished_at       2026-11-16 10:24:53
id                01a1e2d0-bdb7-76e5-827f-4d66628538da
last_error        http_status
last_http_status  500
next_attempt_at   2026-11-16 10:25:53
occurrence        0
status            failed
workspace_id      01a1e2d0-b216-7078-988a-a2bbd9e3357f
attempts:
  NUMBER  AT                   OK  HTTP  ERROR        DURATION (MS)
  1       2026-11-16 10:24:53  no  500   http_status  2
```

A delivery of an alarm also shows `alarm_id`, `alarm_title`, `policy_id`,
`policy_name` and `resource`; a cancelled one shows `detail`, the reason.
With `-o json`, `attempts` holds the attempts as objects (`number`, `at`,
`ok`, `http_status`, `error`, `duration_ms`).

## Silences

A [silence](/docs/concepts/silences/) holds back the notifications of the alarms
it covers for a bounded time. One that starts later is a maintenance
window. Alarms are still evaluated and recorded. What a silence covers is
fixed when you create it; afterwards you can only extend it or end it. For
the tasks, see [Silence alarms and schedule maintenance](/docs/guides/silence-alarms/).

Time flags take RFC 3339 (`2026-11-20T02:00:00Z`, `2026-11-20T04:00+02:00`;
seconds and fractions optional) or epoch milliseconds. Duration flags take
`30m`, `2h`, `1d` or combinations such as `1h30m`.

### forge alerts silences list

Lists silences and maintenance windows, latest start first. With
`alert.view` on the workspace you see every silence; otherwise only those
whose whole scope you may view.

**Synopsis**

```text
forge alerts silences list [--state <STATE>] [--workspace <WORKSPACE>]
    [--limit <N>] [--page <N>] [--all]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--state <STATE>` | `scheduled`, `active` or `expired` | — | no | Only silences in this state, by the server's clock | `--state active` |
| `--workspace <WORKSPACE>` | workspace id | this deployment's workspace | no | Only this workspace's silences | `--workspace <workspace-id>` |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row the server returns | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

The server returns at most 500 silences per request.

**Examples**

```sh
# Every silence
forge alerts silences list
# Silences in effect now
forge alerts silences list --state active
```

**Output**

<!-- TODO: replace with output captured from a running server -->

```text
$ forge alerts silences list
ID                                    STATE      SCOPE                 RULES  SEVERITY          STARTS               ENDS                 REASON
01a1e3f0-1c2d-7a4b-9e10-5f6a7b8c9d01  scheduled  cluster <cluster-id>  all    major and milder  2026-12-01 22:00:00  2026-12-02 02:00:00  Kernel upgrade
01a1e3e8-0b1c-7d2e-8f30-4a5b6c7d8e02  active     nodes <server-id>     all    any               2026-11-16 10:00:00  2026-11-16 12:00:00  Investigating disk usage
showing 1–2 of 2
```

`SCOPE` is `workspace`, `cluster <id>`, `environment <id>`, `nodes <ids>` or
`apps <ids>`. `RULES` is `all` or the rule ids. `SEVERITY` is `any`,
`<min> and worse`, `<max> and milder`, `<min> to <max>`, or one severity.
With `-o json` each row has the [REST silence fields](/docs/reference/rest-api/#silences).

### forge alerts silences show

Shows one silence with every field. Needs `alert.view` on the workspace or
on everything the silence's scope names; otherwise it answers as not found
(exit `6`). `forge alerts silences get` is the same command.

**Synopsis**

```text
forge alerts silences show <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | silence id | — | yes | The silence | `<silence-id>` |

**Examples**

```sh
# One silence
forge alerts silences show <silence-id>
```

**Output**

```text
$ forge alerts silences show <silence-id>
created_at    2026-11-16 09:58:12
created_by    user:<user-id>
ends_at       2026-11-16 12:00:00
id            <silence-id>
reason        Investigating disk usage
rules         []
scope         {"kind":"servers","ids":["<server-id>"]}
starts_at     2026-11-16 10:00:00
state         active
updated_at    2026-11-16 09:58:12
workspace_id  <workspace-id>
```

A silence with severity bounds also shows `min_severity` or
`max_severity`; one ended early shows `ended_by`.

### forge alerts silences create

Creates a silence, or a maintenance window when `--starts-at` is later than
now. Needs `alert.manage` on everything the scope names; audited as
`alert.silence.create`. Name exactly one scope and exactly one of
`--duration` and `--ends-at`.

**Synopsis**

```text
forge alerts silences create (--workspace-wide | --cluster <CLUSTER> | --environment <ENVIRONMENT>
        | --node <NODE>... | --service <SERVICE>...)
    (--duration <DURATION> | --ends-at <TIME>) --reason <REASON>
    [--rule <RULE>...] [--min-severity <SEVERITY>] [--max-severity <SEVERITY>]
    [--starts-at <TIME>] [--workspace <WORKSPACE>]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--workspace-wide` | flag | off | one scope | Every alarm of the workspace. Needs `alert.manage` on the workspace | `--workspace-wide` |
| `--cluster <CLUSTER>` | cluster id | — | one scope | Alarms on the cluster's nodes and the services on them | `--cluster <cluster-id>` |
| `--environment <ENVIRONMENT>` | environment id | — | one scope | Alarms on the environment's services and nodes | `--environment <environment-id>` |
| `--node <NODE>` | node id, host name or name; repeatable, up to 100 | — | one scope | Alarms on the node and the services on it | `--node web-01 --node web-02` |
| `--service <SERVICE>` | service id; repeatable, up to 100 | — | one scope | Alarms on the service | `--service <service-id>` |
| `--duration <DURATION>` | duration, 1m–30d | — | one end | Length from the start | `--duration 2h` |
| `--ends-at <TIME>` | time | — | one end | Exclusive end | `--ends-at 2026-12-02T02:00:00Z` |
| `--reason <REASON>` | string, 1–500 characters | — | yes | Why alarms are quiet. Shown on alarms and in the delivery history | `--reason "Kernel upgrade"` |
| `--rule <RULE>` | rule id; repeatable, up to 100 | every rule | no | Only alarms of this rule | `--rule <rule-id>` |
| `--min-severity <SEVERITY>` | `info`, `warning`, `major` or `critical` | no lower bound | no | Only alarms of this severity and worse | `--min-severity warning` |
| `--max-severity <SEVERITY>` | `info`, `warning`, `major` or `critical` | no upper bound | no | Only alarms of this severity and milder. A worse alarm still notifies | `--max-severity major` |
| `--starts-at <TIME>` | time, up to 365 days ahead | now | no | A later start makes a maintenance window | `--starts-at 2026-12-01T22:00:00Z` |
| `--workspace <WORKSPACE>` | workspace id | this deployment's workspace | no | The workspace | `--workspace <workspace-id>` |

```sh
# Quiet one node for two hours while you investigate
forge alerts silences create --node web-01 --duration 2h --reason "Investigating disk usage"

# A maintenance window on a cluster; critical alarms still notify
forge alerts silences create --cluster <cluster-id> \
  --starts-at 2026-12-01T22:00:00Z --ends-at 2026-12-02T02:00:00Z \
  --max-severity major --reason "Kernel upgrade"
```

**Output:** the new silence, as for
[`forge alerts silences show`](#forge-alerts-silences-show).

### forge alerts silences extend

Moves the end of a silence that has not ended to a later time, at most 30
days after its start. Needs `alert.manage` on everything the scope names;
audited as `alert.silence.extend`. An expired silence cannot be extended
(exit `7`); create a new one.

**Synopsis**

```text
forge alerts silences extend <ID> (--by <DURATION> | --until <TIME>)
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | silence id | — | yes | The silence | `<silence-id>` |
| `--by <DURATION>` | duration | — | one of the two | Time to add to the current end | `--by 1h` |
| `--until <TIME>` | time | — | one of the two | The new end, later than the current one | `--until 2026-12-02T04:00:00Z` |

**Examples**

```sh
# Add an hour to a silence
forge alerts silences extend <silence-id> --by 1h
# Move its end to a fixed time
forge alerts silences extend <silence-id> --until 2026-12-02T04:00:00Z
```

**Output:** the changed silence, as for
[`forge alerts silences show`](#forge-alerts-silences-show).

### forge alerts silences end

Ends a silence now. A maintenance window that has not started never takes
effect. Alarms it covered that still fire notify within seconds, one
message per channel. The silence stays as history. Needs `alert.manage` on
everything the scope names; audited as `alert.silence.end`. Asks for
confirmation on a terminal. A silence that has already ended exits `7`.

**Synopsis**

```text
forge alerts silences end <ID> [--yes]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | silence id | — | yes | The silence | `<silence-id>` |
| `--yes` | flag | off | no | Do not ask for confirmation (required with `--non-interactive`) | `--yes` |

**Examples**

```sh
# End a silence after you confirm
forge alerts silences end <silence-id>
# End it without asking (scripts)
forge alerts silences end <silence-id> --yes
```

**Output:** the ended silence, as for
[`forge alerts silences show`](#forge-alerts-silences-show), with
`state` `expired` and `ended_by` set.
