---
title: forge webhooks
description: Create, rotate and inspect deploy webhooks that let CI/CD redeploy a service or stack.
sidebar:
  order: 9
---

`forge webhooks` manages deploy webhooks (see
[Deploy webhooks](/docs/guides/deploy-webhooks/)). The URL and signing
secret are printed once, at creation and rotation. Changes need
`webhook.manage` with MFA. Every command takes the
[global flags](/docs/reference/cli/#global-flags).

```sh
forge webhooks create ci --service web --signature github --tag-pattern 'v*'
```

### forge webhooks list

Deploy webhooks of the workspace

**Synopsis**

```text
forge webhooks list [OPTIONS]
```

**Examples**

```sh
# Webhooks you can see
forge webhooks list
```

### forge webhooks create

Create one; its URL and signing secret are printed once

**Synopsis**

```text
forge webhooks create [OPTIONS] --service <SERVICE> <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<name>` | string | - | yes | Name | - |
| `--service` | string | - | no | Redeploy this service (id or name) … | - |
| `--stack` | string | - | no | … or this compose stack (id) | - |
| `--signature` | string | `none` | no | none, github, gitlab or forge | - |
| `--tag-pattern` | string | - | no | Tags a delivery may deploy (`v*`); needs a signature | - |
| `--disabled` | flag | - | no | Create or keep it disabled | - |

**Examples**

```sh
# A GitHub-signed webhook that deploys only release tags
forge webhooks create gh-release --service <service> --signature github --tag-pattern 'v*'
# Create it disabled
forge webhooks create staging-hook --service <service> --disabled
```

### forge webhooks update

Change a webhook's name, target, signature or tag pattern

**Synopsis**

```text
forge webhooks update [OPTIONS] --service <SERVICE> <WEBHOOK> <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<webhook>` | string | - | yes | Webhook id or name | - |
| `<name>` | string | - | yes | Name | - |
| `--service` | string | - | no | Redeploy this service (id or name) … | - |
| `--stack` | string | - | no | … or this compose stack (id) | - |
| `--signature` | string | `none` | no | none, github, gitlab or forge | - |
| `--tag-pattern` | string | - | no | Tags a delivery may deploy (`v*`); needs a signature | - |
| `--disabled` | flag | - | no | Create or keep it disabled | - |

**Examples**

```sh
# Change the signature and tag pattern
forge webhooks update <webhook> gh-release --service <service> --signature github --tag-pattern 'v*'
# Disable a webhook
forge webhooks update <webhook> gh-release --service <service> --disabled
```

### forge webhooks delete

Delete a webhook; its URL stops working

**Synopsis**

```text
forge webhooks delete [OPTIONS] <WEBHOOK>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<webhook>` | string | - | yes | Webhook id or name | - |
| `--yes` | flag | - | no | Skip the confirmation prompt | - |

**Examples**

```sh
# Delete a webhook without prompting
forge webhooks delete <webhook> --yes
```

### forge webhooks rotate

New URL and secret; the old ones stop working at once

**Synopsis**

```text
forge webhooks rotate [OPTIONS] <WEBHOOK>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<webhook>` | string | - | yes | Webhook id or name | - |
| `--yes` | flag | - | no | Skip the confirmation prompt | - |

**Examples**

```sh
# Issue a new URL and secret; the old ones stop working
forge webhooks rotate <webhook> --yes
```

### forge webhooks deliveries

Recent deliveries and their outcome

**Synopsis**

```text
forge webhooks deliveries [OPTIONS] <WEBHOOK>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<webhook>` | string | - | yes | Webhook id or name | - |

**Examples**

```sh
# Recent deliveries of a webhook
forge webhooks deliveries <webhook>
```

