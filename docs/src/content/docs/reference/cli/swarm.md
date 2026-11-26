---
title: forge swarm
description: Manage Docker Swarm clusters from the CLI, including nodes, stacks, secrets, configs, networks, volumes, services and mode conversion.
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

```sh
# The Swarm state of a cluster
forge swarm status <cluster>
```

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

```sh
# Make one node the first manager
forge swarm init <cluster> lab-a --wait
# Advertise a fixed address and pick the overlay pool
forge swarm init <cluster> lab-a --advertise-addr 192.0.2.10 --addr-pool 10.20.0.0/16
```

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

```sh
# Join a node as a worker
forge swarm join <cluster> lab-b --wait
# Join a node as a manager
forge swarm join <cluster> lab-c --role manager --wait
```

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

```sh
# Take a node out of the swarm
forge swarm leave <cluster> lab-b --yes --wait
```

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

```sh
# The nodes of the swarm, their roles and availability
forge swarm nodes <cluster>
```

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

```sh
# Make a worker a manager
forge swarm promote <cluster> lab-b
```

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

```sh
# Make a manager a worker
forge swarm demote <cluster> lab-b
```

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

```sh
# Move tasks off a node before maintenance
forge swarm drain <cluster> lab-b
```

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

```sh
# Let a node take tasks again
forge swarm activate <cluster> lab-b
```

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

```sh
# Keep running tasks but schedule no new ones
forge swarm pause <cluster> lab-b
```

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

```sh
# Set node labels used by placement constraints
forge swarm labels <cluster> lab-b --label zone=a --label disk=ssd
# Remove every label
forge swarm labels <cluster> lab-b --clear
```

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

```sh
# Rotate the worker join token
forge swarm tokens <cluster> --worker --yes
```

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

```sh
# The addresses a node can advertise
forge swarm addresses <cluster> lab-a
```

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

```sh
# Run the pre-checks only
forge swarm convert <cluster> --to swarm --check
# Switch the cluster to Swarm mode
forge swarm convert <cluster> --to swarm --yes
```

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

```sh
# Stacks in the cluster and their deployed revision
forge swarm stacks ls <cluster>
```

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

```sh
# Services and current tasks of a stack
forge swarm stacks ps <stack> --environment <environment>
# Include finished tasks
forge swarm stacks ps <stack> --environment <environment> --all
```

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

```sh
# Deploy the latest revision and wait
forge swarm stacks deploy <stack> --environment <environment> --yes --wait
# Deploy a specific revision
forge swarm stacks deploy <stack> --environment <environment> --revision 3 --yes
```

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

```sh
# Remove a stack from the swarm
forge swarm stacks rm <stack> --environment <environment> --yes --wait
```

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

```sh
# What a deploy would change
forge swarm stacks diff <stack> --environment <environment>
# Compare two revisions
forge swarm stacks diff <stack> --environment <environment> --from 2 --to 3
```

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

```sh
# Run three replicas of one service
forge swarm stacks scale <stack> <service> 3 --environment <environment>
```

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

```sh
# Restart the tasks of one service
forge swarm stacks restart <stack> <service> --environment <environment>
```

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

```sh
# Roll one service back to its previous spec
forge swarm stacks rollback <stack> <service> --environment <environment> --yes
```

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

```sh
# The last 200 log lines of one service
forge swarm stacks logs <stack> <service> --environment <environment> --tail 200
```

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

```sh
# Secrets in the cluster and their versions
forge swarm secrets ls <cluster>
```

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

```sh
# Create a secret from standard input
printf '%s' "$DB_PASSWORD" | forge swarm secrets create <cluster> db_password --value-file -
# Create a secret from a credential vault item
forge swarm secrets create <cluster> db_password --from-vault <item-id>
```

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

```sh
# Create the next version from standard input
printf '%s' "$DB_PASSWORD" | forge swarm secrets rotate <cluster> db_password --value-file -
```

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

```sh
# Remove one Docker secret version
forge swarm secrets rm <cluster> <docker-name> --yes
```

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

```sh
# Remove versions no stack uses
forge swarm secrets prune <cluster> --yes
```

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

```sh
# Configs in the cluster and their versions
forge swarm configs ls <cluster>
```

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

```sh
# Create a config from a file
forge swarm configs create <cluster> nginx_conf --value-file ./nginx.conf
```

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

```sh
# Create the next version from a file
forge swarm configs rotate <cluster> nginx_conf --value-file ./nginx.conf
```

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

```sh
# Remove one Docker config version
forge swarm configs rm <cluster> <docker-name> --yes
```

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

```sh
# Remove versions no stack uses
forge swarm configs prune <cluster> --yes
```

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

```sh
# Overlay networks in the swarm
forge swarm networks ls <cluster>
```

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

```sh
# Networks shared between stacks
forge swarm networks shared <cluster>
```

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

```sh
# Volumes across the swarm nodes
forge swarm volumes <cluster>
```

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

```sh
# Resource use of a stack's tasks
forge swarm stats <stack> --environment <environment>
```

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

```sh
# The Swarm state of one service
forge swarm services status <service> --environment <environment>
```

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

```sh
# Deploy one service and wait
forge swarm services deploy <service> --environment <environment> --wait
```

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

```sh
# Run three replicas
forge swarm services scale <service> 3 --environment <environment> --wait
```

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

```sh
# Restart the service's tasks
forge swarm services restart <service> --environment <environment> --wait
```

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

```sh
# Roll back to the previous spec
forge swarm services rollback <service> --environment <environment> --yes --wait
```

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

```sh
# Remove the service from the swarm
forge swarm services remove <service> --environment <environment> --yes --wait
```

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

```sh
# The last 200 log lines
forge swarm services logs <service> --environment <environment> --tail 200
```
