---
title: forge swarm
description: Manage Docker Swarm clusters from the CLI: nodes, stacks, secrets, configs, networks, volumes, services and mode conversion.
sidebar:
  order: 19
---

`forge swarm` gathers every Docker Swarm task. For the steps, see
[Run a Docker Swarm cluster](/docs/guides/docker-swarm/). Every command takes the
[global flags](/docs/reference/cli/#global-flags).

## Permissions

| Commands | Cedar action | Scope | MFA |
| --- | --- | --- | --- |
| `status`, `nodes`, `addresses`, `stacks ls`, `stacks ps`, `stacks logs`, `stacks diff`, `secrets ls`, `configs ls`, `networks ls`, `convert --check` | `view` | the cluster or environment | no |
| `init`, `join`, `leave`, `promote`, `demote`, `drain`, `activate`, `pause`, `labels`, `tokens`, `secrets …`, `configs …`, `networks shared` | `swarm.manage` | the cluster | yes |
| `stacks deploy`, `stacks rm`, `stacks scale`, `stacks restart`, `stacks rollback` | `stack.manage` (and `stack.privileged` for host access) | the environment | only `stack.privileged` |
| `convert` | `workspace.manage` and `swarm.manage` | the cluster | yes |
| `volumes` | `server.manage` | each node | no |
| `services …` | see [forge services](/docs/reference/cli/services/) | | |

## Commands

### forge swarm status

Shows members, live nodes and manager quorum.

**Synopsis**

```text
forge swarm status <CLUSTER>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster id, name or abbreviation |

### forge swarm init

Creates the swarm; the node becomes its first manager. Without `--advertise-addr` the console's suggestion (the mesh address first) is a good choice; see `forge swarm addresses`.

**Synopsis**

```text
forge swarm init [OPTIONS] <CLUSTER> <NODE>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `<node>` | string | Node id, host name or name |
| `--advertise-addr` | string | Address other nodes reach this one on; also used for overlay traffic |
| `--addr-pool` | CIDR | Overlay address pool; repeat |
| `--autolock` | flag | Lock manager keys at rest |
| `--wait` | flag | Wait for the job |
| `--timeout` | seconds | How long `--wait` waits (900) |

### forge swarm join

Adds a node to the swarm as a worker or manager.

**Synopsis**

```text
forge swarm join [OPTIONS] <CLUSTER> <NODE>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `<node>` | string | Node |
| `--role` | manager or worker | Role (default worker) |
| `--advertise-addr` | string | Address to advertise |
| `--wait` | flag | Wait for the job |
| `--timeout` | seconds | How long `--wait` waits |

### forge swarm leave

Removes a node from the swarm.

**Synopsis**

```text
forge swarm leave [OPTIONS] <CLUSTER> <NODE>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `<node>` | string | Node |
| `--force` | flag | Leave even when unreachable or the last manager |
| `--yes` | flag | Skip the confirmation |
| `--wait` | flag | Wait for the job |
| `--timeout` | seconds | How long `--wait` waits |

### forge swarm nodes

Lists the swarm's nodes as a manager sees them.

**Synopsis**

```text
forge swarm nodes <CLUSTER>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |

### forge swarm promote

Makes a worker a manager.

**Synopsis**

```text
forge swarm promote <CLUSTER> <NODE>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `<node>` | string | Node |

### forge swarm demote

Makes a manager a worker.

**Synopsis**

```text
forge swarm demote <CLUSTER> <NODE>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `<node>` | string | Node |

### forge swarm drain

Moves the node's tasks to other nodes and schedules nothing new on it.

**Synopsis**

```text
forge swarm drain <CLUSTER> <NODE>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `<node>` | string | Node |

### forge swarm activate

Schedules tasks on the node again.

**Synopsis**

```text
forge swarm activate <CLUSTER> <NODE>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `<node>` | string | Node |

### forge swarm pause

Keeps the node's tasks and schedules nothing new on it.

**Synopsis**

```text
forge swarm pause <CLUSTER> <NODE>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `<node>` | string | Node |

### forge swarm labels

Replaces a node's labels.

**Synopsis**

```text
forge swarm labels [OPTIONS] <CLUSTER> <NODE>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `<node>` | string | Node |
| `--label` | KEY=VALUE | Label; repeat |
| `--clear` | flag | Remove every label |

### forge swarm tokens

Rotates the join tokens.

**Synopsis**

```text
forge swarm tokens [OPTIONS] <CLUSTER>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `--worker` | flag | Rotate the worker token |
| `--manager` | flag | Rotate the manager token |
| `--yes` | flag | Skip the confirmation |

### forge swarm addresses

Lists the addresses a node could advertise: the mesh address first, then its primary and interface addresses.

**Synopsis**

```text
forge swarm addresses <CLUSTER> <NODE>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `<node>` | string | Node |

### forge swarm convert

Switches a cluster between Default and Swarm mode after the pre-checks pass.

**Synopsis**

```text
forge swarm convert [OPTIONS] --to <TO> <CLUSTER>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `--to` | default or swarm | Target mode |
| `--check` | flag | Only run the checks |
| `--yes` | flag | Skip the confirmation |

### forge swarm stacks ls

Lists stacks in the cluster's environments and the revision deployed.

**Synopsis**

```text
forge swarm stacks ls <CLUSTER>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |

### forge swarm stacks ps

Shows services, replicas, ports and current tasks of a stack.

**Synopsis**

```text
forge swarm stacks ps [OPTIONS] <STACK>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<stack>` | string | Stack id or name |
| `--environment` | string | The stack's environment |
| `--all` | flag | Every task, not only the newest per replica |

### forge swarm stacks deploy

Deploys a revision with `docker stack deploy`. An older revision rolls back.

**Synopsis**

```text
forge swarm stacks deploy [OPTIONS] <STACK>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<stack>` | string | Stack |
| `--environment` | string | Environment |
| `--revision` | integer | Revision (default: latest) |
| `--yes` | flag | Skip the confirmation |
| `--wait` | flag | Wait for the job |
| `--timeout` | seconds | How long `--wait` waits |

### forge swarm stacks rm

Removes the stack from the swarm. Volumes stay.

**Synopsis**

```text
forge swarm stacks rm [OPTIONS] <STACK>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<stack>` | string | Stack |
| `--environment` | string | Environment |
| `--yes` | flag | Skip the confirmation |
| `--wait` | flag | Wait for the job |
| `--timeout` | seconds | How long `--wait` waits |

### forge swarm stacks diff

Prints line differences between two revisions (default: deployed to latest).

**Synopsis**

```text
forge swarm stacks diff [OPTIONS] <STACK>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<stack>` | string | Stack |
| `--environment` | string | Environment |
| `--from` | integer | Older revision |
| `--to` | integer | Newer revision |

### forge swarm stacks scale

Sets a stack service's replica count.

**Synopsis**

```text
forge swarm stacks scale [OPTIONS] <STACK> <SERVICE> <REPLICAS>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<stack>` | string | Stack |
| `--environment` | string | Environment |
| `<service>` | string | Compose service name |
| `<replicas>` | integer | 0 to 1000 |

### forge swarm stacks restart

Replaces every task of a stack service.

**Synopsis**

```text
forge swarm stacks restart [OPTIONS] <STACK> <SERVICE>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<stack>` | string | Stack |
| `--environment` | string | Environment |
| `<service>` | string | Service |

### forge swarm stacks rollback

Returns a stack service to its previous spec.

**Synopsis**

```text
forge swarm stacks rollback [OPTIONS] <STACK> <SERVICE>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<stack>` | string | Stack |
| `--environment` | string | Environment |
| `<service>` | string | Service |
| `--yes` | flag | Skip the confirmation |

### forge swarm stacks logs

Prints a stack service's last log lines.

**Synopsis**

```text
forge swarm stacks logs [OPTIONS] <STACK> <SERVICE>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<stack>` | string | Stack |
| `--environment` | string | Environment |
| `<service>` | string | Service |
| `--tail` | integer | Lines (200) |

### forge swarm secrets ls

Lists every version of the swarm's secrets and the services using each.

**Synopsis**

```text
forge swarm secrets ls <CLUSTER>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |

### forge swarm secrets create

Creates version 1. Read the value from a file or standard input, never from an argument.

**Synopsis**

```text
forge swarm secrets create [OPTIONS] <CLUSTER> <NAME>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `<name>` | string | a-z, 0-9, `_`, `-` |
| `--value-file` | path | File with the value; `-` for standard input |
| `--from-vault` | string | Credential vault item id |

### forge swarm secrets rotate

Creates the next version. Redeploy the stacks that use it.

**Synopsis**

```text
forge swarm secrets rotate [OPTIONS] <CLUSTER> <NAME>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `<name>` | string | Name |
| `--value-file` | path | File with the value |
| `--from-vault` | string | Credential vault item id |

### forge swarm secrets rm

Removes one version (`<name>.v<N>`) no service uses.

**Synopsis**

```text
forge swarm secrets rm [OPTIONS] <CLUSTER> <DOCKER_NAME>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `<docker-name>` | string | For example `db_password.v1` |
| `--yes` | flag | Skip the confirmation |

### forge swarm secrets prune

Removes old versions no service uses.

**Synopsis**

```text
forge swarm secrets prune [OPTIONS] <CLUSTER>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `--yes` | flag | Skip the confirmation |

### forge swarm configs ls

Lists every version of the swarm's configs and the services using each.

**Synopsis**

```text
forge swarm configs ls <CLUSTER>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |

### forge swarm configs create

Creates version 1. Read the value from a file or standard input, never from an argument.

**Synopsis**

```text
forge swarm configs create [OPTIONS] <CLUSTER> <NAME>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `<name>` | string | a-z, 0-9, `_`, `-` |
| `--value-file` | path | File with the value; `-` for standard input |
| `--from-vault` | string | Credential vault item id |

### forge swarm configs rotate

Creates the next version. Redeploy the stacks that use it.

**Synopsis**

```text
forge swarm configs rotate [OPTIONS] <CLUSTER> <NAME>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `<name>` | string | Name |
| `--value-file` | path | File with the value |
| `--from-vault` | string | Credential vault item id |

### forge swarm configs rm

Removes one version (`<name>.v<N>`) no service uses.

**Synopsis**

```text
forge swarm configs rm [OPTIONS] <CLUSTER> <DOCKER_NAME>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `<docker-name>` | string | For example `db_password.v1` |
| `--yes` | flag | Skip the confirmation |

### forge swarm configs prune

Removes old versions no service uses.

**Synopsis**

```text
forge swarm configs prune [OPTIONS] <CLUSTER>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |
| `--yes` | flag | Skip the confirmation |

### forge swarm networks ls

Lists swarm networks with subnets, encryption and attached services. `ingress` is the routing mesh.

**Synopsis**

```text
forge swarm networks ls <CLUSTER>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |

### forge swarm networks shared

Creates the shared network `forge-shared` if it is missing.

**Synopsis**

```text
forge swarm networks shared <CLUSTER>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |

### forge swarm volumes

Lists the Docker volumes on every node of the swarm and the containers using them.

**Synopsis**

```text
forge swarm volumes <CLUSTER>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<cluster>` | string | Cluster |

### forge swarm stats

Shows the CPU and memory of a stack's running tasks, sampled now on the nodes that run them.

**Synopsis**

```text
forge swarm stats [OPTIONS] <STACK>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<stack>` | string | Stack id or name |
| `--environment` | string | The stack's environment |

### forge swarm services status

Shows a Swarm service's settings, tasks and HA report.

**Synopsis**

```text
forge swarm services status [OPTIONS] <SERVICE>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<service>` | string | Service id or name |
| `--environment` | string | The service's environment |

### forge swarm services deploy

Applies the current definition to the swarm.

**Synopsis**

```text
forge swarm services deploy [OPTIONS] <SERVICE>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<service>` | string | Service id or name |
| `--environment` | string | The service's environment |
| `--wait` | flag | Wait for the job |
| `--timeout` | seconds | How long `--wait` waits |

### forge swarm services scale

Sets the replica count.

**Synopsis**

```text
forge swarm services scale [OPTIONS] <SERVICE> <REPLICAS>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<service>` | string | Service id or name |
| `--environment` | string | The service's environment |
| `<replicas>` | integer | 0 to 1000 |
| `--wait` | flag | Wait for the job |
| `--timeout` | seconds | How long `--wait` waits |

### forge swarm services restart

Replaces every task.

**Synopsis**

```text
forge swarm services restart [OPTIONS] <SERVICE>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<service>` | string | Service id or name |
| `--environment` | string | The service's environment |
| `--wait` | flag | Wait for the job |
| `--timeout` | seconds | How long `--wait` waits |

### forge swarm services rollback

Returns to the previous release.

**Synopsis**

```text
forge swarm services rollback [OPTIONS] <SERVICE>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<service>` | string | Service id or name |
| `--environment` | string | The service's environment |
| `--yes` | flag | Skip the confirmation |
| `--wait` | flag | Wait for the job |
| `--timeout` | seconds | How long `--wait` waits |

### forge swarm services remove

Removes the Swarm service; the Forge service stays.

**Synopsis**

```text
forge swarm services remove [OPTIONS] <SERVICE>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<service>` | string | Service id or name |
| `--environment` | string | The service's environment |
| `--yes` | flag | Skip the confirmation |
| `--wait` | flag | Wait for the job |
| `--timeout` | seconds | How long `--wait` waits |

### forge swarm services logs

Prints the last log lines of all tasks.

**Synopsis**

```text
forge swarm services logs [OPTIONS] <SERVICE>
```

**Arguments and flags**

| Name | Type | Description |
| --- | --- | --- |
| `<service>` | string | Service id or name |
| `--environment` | string | The service's environment |
| `--tail` | integer | Lines (200) |
