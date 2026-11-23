---
title: Plugin manifest
description: forge-plugin.toml fields, limits and validation rules.
sidebar:
  order: 5
---

Unknown keys are rejected.

## `[plugin]`

| Key | Required | Rules |
| --- | --- | --- |
| `id` | yes | lowercase `publisher.name`, `[a-z0-9.-]`, ≤ 64 chars |
| `name` | yes | display name |
| `version` | yes | semver |
| `publisher` | yes | must be a trusted publisher |
| `forge_api` | yes | semver requirement matched against the host's plugin API (0.1.0) |
| `description` | no | |

## `[runtime]`

| Key | Default | Notes |
| --- | --- | --- |
| `export` | `run` | exported function `func(u32) -> u32` |
| `location` | `central` | `central`, `worker` (Phase 3), `node` (Phase 9) |
| `targets` | `[]` | OS/arch list for node plugins |

## `[permissions]`

`requested = ["host:log", "host:server-online"]` — admins approve a subset at
install time.

## `[budgets]`

| Key | Default | Maximum |
| --- | --- | --- |
| `fuel` | 50 000 000 | 10 000 000 000 |
| `memory_mb` | 16 | 512 |
| `timeout_ms` | 2000 | 60 000 |
| `max_output_bytes` | 65 536 | — |
| `concurrency` | 2 | — |

## `[dependencies]`

`"other.plugin" = ">=1.2, <2"` — must be installed and satisfy the range;
dependents block uninstalling.

## `[config]`

Free-form schema for user configuration, rendered by the UI (later phases).
