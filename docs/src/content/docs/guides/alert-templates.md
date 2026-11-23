---
title: Create alerts from templates
description: Add ready-made alert rules to a node, a service, a database or an ingress certificate from where you see it, or with the CLI.
sidebar:
  order: 23
---

This guide adds alert rules from templates: ready-made conditions such as
"disk almost full", "liveness failing" or "database stopped". You start
from the node, service, database or certificate you want to watch, and the
rule comes scoped to it. For every template and its condition, see
[Alert rule templates](/docs/reference/alert-rule-templates/). For how rules
behave, see [Alerts](/docs/concepts/alerts/).

## Before you begin

- You need `alert.manage` on the workspace (**Operator**, **Workspace
  admin** or **Platform admin**) to create rules. `alert.view` is enough to
  list templates and subjects.
- Node templates need an enrolled node. Disk, memory, swap and uptime work
  on Linux and Windows nodes; `node-load-high` needs a Linux node.
- Service health templates need a health check on the service. See
  [Health checks](/docs/concepts/health-checks/).
- Database templates need a package installed from the App Store.
- Certificate templates need an uploaded ingress certificate. Let's Encrypt
  certificates renew themselves and are not covered.
- A rule only opens alarms. To be told about them, set up a
  [notification policy](/docs/guides/alarm-notifications/).

## Add an alert to a node

1. Open **Nodes** and select the node.
2. Select **Add alert** at the top of the page.
3. Under **From template**, pick a template, for example **Disk filling up**.
   **Watches** is already **Nodes**, with this node selected.
4. Change the name, severity or condition if you want.
5. Select **Save**.

With the CLI:

```sh
forge alerts create-rule --template node-disk-filling --node <node-name>
```

## Add an alert to a service

1. Open **Services** (under **Services**) and select the service.
2. Select the **Health Checks** tab.
3. Do one of these:
   - In a check's row menu, select **Alert when this fails**. The dialog
     opens with the template for the check's purpose (liveness, readiness or
     startup) and the check's name in the subject.
   - Select **Add alert** above the checks and pick a template, for example
     **Instance failed** or **Deployment failed**. For a health template,
     replace `{check}` in **Subject** with the check's name.
4. Select **Save**.

With the CLI:

```sh
# The "http" liveness check of the shop service
forge alerts create-rule --template service-liveness-failing --app <app-name> --check http

# Any instance of the service reporting failed
forge alerts create-rule --template service-instance-failed --app <app-name>
```

## Add an alert to a database

Databases are packages, so these steps work for any installed package.

1. Open the node's **Packages** tab, or the package's page in the App
   Store.
2. Open the row menu of the installation and select **Add alert**.
3. Under **From template**, pick a template. **Database stopped** and
   **Install or upgrade failed** watch the installation; the two
   **Database host** templates watch the node it runs on.
4. Select **Save**.

With the CLI:

```sh
forge alerts create-rule --template database-stopped --package postgresql
forge alerts create-rule --template database-host-disk-filling --package postgresql
```

When the package is installed on several nodes, pass the installation id
instead of the name. `forge api call GET /api/v1/installations` lists them.

## Add an alert to a certificate

1. Open **Ingress** and select **Certificates**.
2. Under **Uploaded**, select **Alert** on the certificate.
3. Keep **Certificate expires within 14 days**, or pick the 3-day template.
4. Select **Save**.

With the CLI:

```sh
forge alerts create-rule --template certificate-expires-14d --certificate <certificate-name>
forge alerts create-rule --template certificate-expires-3d --certificate <certificate-name> --severity critical
```

## Verify

1. Open **Operations → Alerts** and select **Rules**. The new rule is listed
   with its subject.
2. Run `forge alerts rules` to see it from the CLI.
3. When the condition holds, an alarm appears under **Alarms**. An alarm on
   a package shows as **package on** its node; one on a certificate as
   **certificate** and its id.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| Saving answers "unknown subject … did you mean" | The subject has a typo | Use the suggested subject, or pick one from the **Subject** list. `forge alerts subjects` lists every subject |
| Saving answers "applies to a … scope" | The subject belongs to another kind of resource, for example `package.state` on nodes | Change **Watches** to the kind the subject names |
| Saving says to replace `{check}` | A health template needs a check name | Put the check's name in **Subject**, or use **Alert when this fails** on the check |
| `create-rule` exits with `2`: "does not apply to template" | A target flag does not fit the template's scope | Use `--node` for node templates, `--app` for service templates, `--package` for database templates and `--certificate` for certificate templates |
| `--package` answers "matches 2 installations" | The package is installed on more than one node | Pass the installation id |
| A load rule never fires | The node runs Windows, or an agent that does not report its CPU count | Use the CPU templates instead |
| A rule never fires although the problem is visible | No current evidence: the node is offline or the agent is old | Check **Operations → Alerts** for a node-offline alarm, and [update the agent](/docs/guides/update-agents/) |

## Next steps

- [Alert rule templates](/docs/reference/alert-rule-templates/): every template
  with its full condition
- [Send alarm notifications](/docs/guides/alarm-notifications/)
- [Silence alarms during maintenance](/docs/concepts/silences/)
- [`forge alerts`](/docs/reference/cli/alerts/)
