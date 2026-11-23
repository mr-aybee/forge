---
title: Alert rule templates
description: Every ready-made alert rule Forge offers, with its subject, scope, severity and full condition.
sidebar:
  order: 9
---

Forge ships ready-made alert rules for nodes, services, databases and
certificates. This page lists every template with its full condition. The
server holds the one definition: `GET /api/v1/alert-rule-templates` and
`forge alerts templates` return the same list. To use them, see
[Create alerts from templates](/docs/guides/alert-templates/). For what each
subject means, see the [subjects table](/docs/concepts/alerts/#subjects).

A rule made from a template is an ordinary rule: you can change its name,
severity, scope and condition before or after you save it.

## Fields

| Field | Type | Description |
| --- | --- | --- |
| `id` | string | Stable identifier, such as `node-disk-filling` |
| `place` | `node`, `service`, `database` or `certificate` | Where the console offers it |
| `name` | string | The rule name it suggests |
| `description` | string | What it catches |
| `severity` | `info`, `warning`, `major` or `critical` | The alarm severity it suggests |
| `subject` | string | The subject it watches. `{check}` is replaced with a health check name |
| `scope` | `servers`, `services`, `installations` or `certificates` | The kind of resource the rule watches |
| `spec` | object | The condition: `{trigger, mode, recovery}`, as in the [rule body](/docs/reference/rest-api/#alerts) |

Durations below are in the units the API uses: milliseconds. `1 min` is
`60000`.

## Node templates

Scope `servers`. In the console: the node page, **Add alert**.

| ID | Name | Severity | Subject | Trigger | Mode | Recovery |
| --- | --- | --- | --- | --- | --- | --- |
| `node-offline` | Node offline | critical | `node.agent.status` | state in `offline` | sustained, `for_ms` `120000` | trigger no longer holds, hold `0` |
| `node-never-connected` | Node never connected | warning | `node.agent.status` | state in `pending` | sustained, `for_ms` `1800000` | trigger no longer holds, hold `0` |
| `node-cpu-high` | High CPU | warning | `node.cpu.percent` | `gt` `90` | window average, `window_ms` `600000`, `min_coverage_pct` `80` | `lt` `80`, hold `300000` |
| `node-cpu-spikes` | CPU spikes | warning | `node.cpu.percent` | `ge` `95` | repeated, `count` `5`, `window_ms` `1800000`, `min_spacing_ms` `60000` | trigger no longer met, hold `0` |
| `node-memory-pressure` | Memory pressure | major | `node.mem.used_percent` | `gt` `90` | sustained, `for_ms` `600000` | `lt` `85`, hold `0` |
| `node-disk-filling` | Disk filling up | warning | `node.disk.max_used_percent` | `gt` `85` | sustained, `for_ms` `300000` | trigger no longer holds, hold `0` |
| `node-disk-almost-full` | Disk almost full | critical | `node.disk.max_used_percent` | `gt` `95` | sustained, `for_ms` `120000` | trigger no longer holds, hold `0` |
| `node-swap-in-use` | Swap in use | warning | `node.swap.used_percent` | `gt` `50` | sustained, `for_ms` `600000` | trigger no longer holds, hold `0` |
| `node-load-high` | Load high | warning | `node.load.per_cpu` | `gt` `2` | sustained, `for_ms` `600000` | trigger no longer holds, hold `0` |
| `node-rebooted` | Node rebooted | info | `node.uptime.seconds` | `lt` `600` | immediate | trigger no longer holds (after 10 minutes of uptime), hold `0` |

:::note
`node.load.per_cpu` is unknown on Windows nodes, so `node-load-high` never
fires there. `node-rebooted` resolves by itself ten minutes after the boot.
:::

## Service templates

Scope `services`, one series per instance (the service on one node).
In the console: the service's **Health Checks** tab, **Add alert**, or
**Alert when this fails** in a check's menu, which fills `{check}`.

| ID | Name | Severity | Subject | Trigger | Mode | Recovery |
| --- | --- | --- | --- | --- | --- | --- |
| `service-liveness-failing` | Liveness failing | critical | `health.liveness.{check}` | state in `failing` | sustained, `for_ms` `60000` | trigger no longer holds, hold `0` |
| `service-readiness-failing` | Readiness failing | major | `health.readiness.{check}` | state in `failing` | sustained, `for_ms` `60000` | trigger no longer holds, hold `0` |
| `service-startup-exhausted` | Startup failed | critical | `health.startup.{check}` | state in `exhausted` | immediate | trigger no longer holds, hold `0` |
| `service-health-flapping` | Health flapping | warning | `health.readiness.{check}` | state in `failing` | repeated, `count` `3`, `window_ms` `900000`, `min_spacing_ms` `0` | trigger no longer met, hold `0` |
| `service-instance-failed` | Instance failed | critical | `app.instance.state` | state in `failed` | sustained, `for_ms` `60000` | trigger no longer holds, hold `0` |
| `service-deploy-failed` | Deployment failed | major | `app.deploy.status` | state in `failed` | immediate | trigger no longer holds, hold `0` |

## Database templates

In the console: **Add alert** in a package's row menu, on the node's
**Packages** tab or the package page. Databases are package installations,
so these templates work for any package.

| ID | Name | Severity | Scope | Subject | Trigger | Mode | Recovery |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `database-stopped` | Database stopped | critical | `installations` | `package.state` | state in `stopped` | sustained, `for_ms` `120000` | trigger no longer holds, hold `0` |
| `database-operation-failed` | Install or upgrade failed | major | `installations` | `package.status` | state in `failed` | immediate | trigger no longer holds, hold `0` |
| `database-host-disk-filling` | Database host disk filling up | warning | `servers` | `node.disk.max_used_percent` | `gt` `85` | sustained, `for_ms` `300000` | trigger no longer holds, hold `0` |
| `database-host-memory-pressure` | Database host memory pressure | major | `servers` | `node.mem.used_percent` | `gt` `90` | sustained, `for_ms` `600000` | `lt` `85`, hold `0` |

The two host templates watch the node the database runs on: started from a
package, they are scoped to that installation's node.

:::note
There is no "port not answering" template. Forge has no probe that tests a
database's own port yet; see the note under
[Subjects](/docs/concepts/alerts/#subjects).
:::

## Certificate templates

Scope `certificates`: uploaded ingress certificates. In the console:
**Ingress** → **Certificates**, **Alert** on a certificate.

| ID | Name | Severity | Subject | Trigger | Mode | Recovery |
| --- | --- | --- | --- | --- | --- | --- |
| `certificate-expires-14d` | Certificate expires within 14 days | warning | `tls.cert.expires_in_days` | `lt` `14` | immediate | trigger no longer holds, hold `0` |
| `certificate-expires-3d` | Certificate expires within 3 days | critical | `tls.cert.expires_in_days` | `lt` `3` | immediate | trigger no longer holds, hold `0` |

Replacing the certificate with a renewed one resolves the alarm on the next
pass.

## Example

The `node-cpu-high` template as the API returns it:

```json
{
  "id": "node-cpu-high",
  "place": "node",
  "name": "High CPU",
  "description": "CPU averages above 90 % over 10 minutes; clears below 80 % for 5 minutes.",
  "severity": "warning",
  "subject": "node.cpu.percent",
  "scope": "servers",
  "spec": {
    "trigger": { "kind": "threshold", "cmp": "gt", "value": 90.0 },
    "mode": { "mode": "window_average", "window_ms": 600000, "min_coverage_pct": 80 },
    "recovery": { "condition": { "kind": "threshold", "cmp": "lt", "value": 80.0 }, "hold_ms": 300000 }
  }
}
```

## Related

- [Alerts](/docs/concepts/alerts/): rules, series, time conditions and subjects
- [Create alerts from templates](/docs/guides/alert-templates/)
- [`forge alerts templates`](/docs/reference/cli/alerts/#forge-alerts-templates)
- [REST API: Alerts](/docs/reference/rest-api/#alerts)
