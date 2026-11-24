---
title: forge registries
description: Add, list, update and remove the container registries whose stored credentials nodes use to pull and push images.
sidebar:
  order: 9
---

`forge registries` manages the workspace's container registries (see
[Registries](/docs/guides/registries/)). Credentials are sealed on the
server, never shown again and read only from stdin. Changes need
`registry.manage` with MFA. Every command takes the
[global flags](/docs/reference/cli/#global-flags).

```sh
printf %s "$GHCR_TOKEN" | forge registries add ghcr --kind ghcr --host ghcr.io --username <user> --secret-stdin
```

### forge registries list

Registries of the workspace (credentials are never shown)

**Synopsis**

```text
forge registries list [OPTIONS]
```

### forge registries add

Add a registry: `printf %s "$TOKEN" | forge registries add ghcr --kind ghcr --host ghcr.io --username me --secret-stdin`

**Synopsis**

```text
forge registries add [OPTIONS] --kind <KIND> --host <HOST> --username <USERNAME> <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<name>` | string | - | yes | Name | - |
| `--kind` | enum: docker-hub \| ghcr \| gitlab \| ecr \| custom | - | no | docker-hub, ghcr, gitlab, ecr or custom (required) | - |
| `--host` | string | - | no | `host[:port]`, e.g. `ghcr.io` | - |
| `--username` | string | - | no | User name (required) | - |
| `--secret-stdin` | flag | - | no | Read the password or token from stdin (never from a flag) | - |
| `--allow-push` | flag | - | no | Images may be pushed to it | - |

### forge registries update

Replace a registry's settings; without `--secret-stdin` the stored secret stays (only while host, kind and user name stay the same)

**Synopsis**

```text
forge registries update [OPTIONS] --kind <KIND> --host <HOST> --username <USERNAME> <REGISTRY> <NAME>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<registry>` | string | - | yes | Registry id or name | - |
| `<name>` | string | - | yes | Name | - |
| `--kind` | enum: docker-hub \| ghcr \| gitlab \| ecr \| custom | - | no | docker-hub, ghcr, gitlab, ecr or custom (required) | - |
| `--host` | string | - | no | `host[:port]`, e.g. `ghcr.io` | - |
| `--username` | string | - | no | User name (required) | - |
| `--secret-stdin` | flag | - | no | Read the password or token from stdin (never from a flag) | - |
| `--allow-push` | flag | - | no | Images may be pushed to it | - |

### forge registries remove

Remove a registry and its stored credentials

**Synopsis**

```text
forge registries remove [OPTIONS] <REGISTRY>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<registry>` | string | - | yes | Registry id or name | - |
| `--yes` | flag | - | no | Skip the confirmation prompt | - |

