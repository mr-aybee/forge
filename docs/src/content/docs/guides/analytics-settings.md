---
title: Configure analytics retention and limits
description: Change how long analytics data is kept and how much CPU, memory and disk the analytics engine and log searches may use, without restarting Forge.
sidebar:
  order: 61
---

This guide shows you how to change analytics retention, the embedded
engine's CPU cap and memory budget, log search limits, the ingest queue and the
free-disk floor while Forge runs. For how these settings work, see
[Analytics and the two databases](/docs/concepts/analytics/#runtime-settings-and-resource-limits).

## Before you begin

- You need the `platform_admin` role. Changing settings and restarting the
  engine uses the Cedar action `analytics.configure` on the platform, which
  sits under the MFA guardrail: sign in with MFA or step up when asked. An
  API key must be **elevated** (with `[security] allow_elevated_tokens` on);
  see [MFA](/docs/guides/mfa/#which-actions-need-mfa). Reading them
  needs only `view` on the platform.
- Analytics must be on: `[analytics] engine` is `embedded` or `clickhouse`.
  See [Choose and connect databases](/docs/guides/databases/).
- The CPU cap and memory budget apply only to the embedded engine, which
  runs on Linux only. They need no extra privileges or systemd settings.
  With an external ClickHouse (`engine = "clickhouse"`) the console hides
  them and the API refuses changes; set CPU and memory on the ClickHouse
  host instead.

:::note
`forge.toml` supplies the defaults
([Configuration: `[analytics]`](/docs/reference/configuration/#analytics)).
What you set here is an override stored in the operational database. It
survives restarts and upgrades and wins over `forge.toml` until you reset it.
:::

## Change a setting

1. Open **Administration → Settings** and select the **Analytics** tab.
   Each setting shows its `forge.toml` value, your override and the value
   in effect, with the engine's current CPU, memory and free disk.
2. Change the values you want. Leave a field empty, or reset it, to go back
   to the `forge.toml` value. `0` turns off the CPU cap and the search
   lookback, and keeps the engine default for search threads and memory.
   **Minute-level history** also sets how long per-minute container metrics
   (`container_metrics_1m`) are kept, and **Hourly rollups** sets it for
   hourly container metrics (`container_metrics_1h`).
   The **CPU cap** rounds up to whole CPUs: `1.5` pins the engine to 2 CPUs.
3. Click **Save**. If you shorten a retention period, Forge asks you to
   confirm.

   :::caution
   Shortening retention makes existing data older than the new limit
   eligible for deletion. ClickHouse removes it in background merges; you
   cannot recover it.
   :::

4. Read the result per setting. `applied` means the engine or Forge reports
   the new value. Any other status comes with a reason; see
   [Troubleshooting](#troubleshooting).

The same change with the API:

```sh
curl -sS -X PUT https://<forge-host>/api/v1/analytics/settings \
  -H "Authorization: Bearer $FORGE_TOKEN" -H 'Content-Type: application/json' \
  -d '{"settings":{"app_logs_days":14,"memory_budget_mb":2048},"confirm":{"shorten_retention":true}}'
```

Send `null` for a setting to remove its override. Every setting, its unit
and its bounds are listed in
[REST API: Analytics settings](/docs/reference/rest-api/#analytics-settings).
Each change is audited as `analytics.settings.update`.

## Restart the engine for a new memory budget or CPU cap

The embedded engine usually picks up a new memory budget on its own within
about 15 seconds. When it does not, the setting reports `pending_restart`.
A new CPU cap pins the engine at once, but its thread pools resize only
when it restarts, so the console shows the restart banner.

1. On the **Analytics** tab, in the **Pending analytics restart** banner,
   click **Restart now**, or click **Schedule…** and pick a time up to
   7 days ahead.
2. To call off a scheduled restart, click **Cancel** in the same banner.

With the API, `POST /api/v1/analytics/restart` restarts now, and
`{"at": <epoch-ms>}` schedules it. `DELETE /api/v1/analytics/restart`
cancels the schedule. These are audited as `analytics.restart` and
`analytics.restart.cancel`.

Forge itself stays up during the restart. Node history and log searches
report analytics as unavailable for a few seconds, and log ingest answers
`503` until the engine is back; senders that retry lose nothing.

## Verify

- The **Analytics** tab shows the new value as in effect. With a CPU cap,
  the **CPU** cell shows the CPUs the engine may run on after the `/`.
- With the API:

  ```sh
  curl -sS https://<forge-host>/api/v1/analytics/settings \
    -H "Authorization: Bearer $FORGE_TOKEN"
  ```

  Each entry in `settings` has the new `effective` value,
  `pending_restart` is `false`, and `last_apply` lists every setting as
  `applied`.
- **Administration → Audit log** has an `analytics.settings.update` entry
  with the old and new values.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `cpu_quota_vcpu` is `failed`: "cannot set the analytics engine's CPU affinity …" | The kernel refused the affinity, for example because Forge itself may use fewer CPUs than the cap | Lower the cap, or check the CPUs Forge may use: `grep Cpus_allowed_list /proc/$(pidof forge-server)/status` |
| `cpu_quota_vcpu` is `pending_restart`: "applied when the engine starts" | The engine is not running | Nothing to do; it is pinned when it starts |
| `memory_budget_mb` is `pending_restart` | The engine did not pick up the new budget | Restart the engine now or schedule it; see [Restart the engine](#restart-the-engine-for-a-new-memory-budget-or-cpu-cap) |
| Saving answers `409` `managed_externally` | `engine = "clickhouse"` (or analytics is off): CPU and memory belong to the ClickHouse host | Set them on that host, for example `max_server_memory_usage` and `max_threads` in its configuration |
| The log says "`[analytics] memory_limit_mb` is no longer used" | `forge.toml` still sets the removed hard memory limit | Delete the line; use `embedded_memory_mb` for the engine's memory cap |
| Retention is `failed`: "the analytics database refused the change" | With an external ClickHouse, Forge's user lacks `ALTER` on the analytics tables | Grant `ALTER` on the analytics database's tables to the user in `[analytics] url`, then save again |
| Retention is `pending` | The analytics database or the analytics worker is unreachable, or analytics is off | The stored value applies when the database is reachable, the worker runs again, or analytics is turned on |
| Saving answers `403` `step_up_required` | The session has no MFA verification | Step up with your code or passkey, then save again |
| Log ingest answers `503 ingest_disk_low` | The engine has less free disk than `min_free_disk_mb` | Free disk, or shorten retention so merges drop old parts |
| Log ingest answers `503 ingest_busy` | More request bodies are queued than `ingest_max_queued_mb`, or over 4,096 requests | Raise `ingest_max_queued_mb`, or have senders batch more |
| Log searches answer `503 search_busy` | `search_max_concurrent` searches are already running | Retry, or raise the limit if the engine has CPU to spare |
| Analytics answers `503 analytics_worker_down`; the **Analytics worker** panel shows `restarting` | The worker exited, stopped answering or passed `worker_memory_limit_mb`, and Forge is restarting it | Wait a few seconds. If **Last restart** keeps saying **memory limit**, raise `[analytics] worker_memory_limit_mb` and restart forge-server; each restart is in the audit log as `analytics.worker.restart` |
| The **Analytics worker** panel shows `restarting` with "cannot start …/forge-analytics-worker" | The worker binary is missing, for example after a self-update from an older release | Install the release package again (`install.sh`), or set `[analytics] worker_binary` |
| The **Analytics worker** panel shows `incompatible` | The worker binary is from another release than forge-server | Install the matching release, then restart forge-server |

## Next steps

- [Analytics and the two databases](/docs/concepts/analytics/) explains
  retention and how the engine's CPU and memory limits work.
- [Configuration: `[analytics]`](/docs/reference/configuration/#analytics)
  lists the `forge.toml` defaults.
- [REST API: Analytics settings](/docs/reference/rest-api/#analytics-settings)
  documents every field and status.
- [Send and search service logs](/docs/guides/application-logs/).
