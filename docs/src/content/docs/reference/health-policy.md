---
title: Health policy
description: Every field of a service's health policy, with types, defaults, limits and examples.
sidebar:
  order: 40
---

This page lists every field of a service's health policy. The console's
form and its YAML view edit the same object, and the API takes it as JSON.
For what the checks mean, see [Health checks](/docs/concepts/health-checks/).

Unknown fields are refused everywhere.

## Policy

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `default_checks` | boolean | `true` | Forge's [default checks](/docs/concepts/health-checks/#default-checks). Written out only when `false`. A configured readiness check replaces the default readiness |
| `startup` | list of [checks](#check) | empty | At most 10 |
| `readiness` | list of checks | empty | At most 10 |
| `liveness` | list of checks | empty | At most 10 |

## Check

| Field | Type | Default | Limits | Notes |
| --- | --- | --- | --- | --- |
| `name` | string | required | `a-z`, `0-9`, `-`; at most 40; unique within its purpose | Used in the subject `health.<purpose>.<name>` |
| `enabled` | boolean | `true` | | |
| `required` | boolean | `true` | `false` only for readiness | An optional readiness check that fails shows "degraded" |
| `probe` | [probe](#probe) | required | | |
| `period_secs` | integer, seconds | `10` | 1–3600 | Time between attempts |
| `timeout_secs` | integer, seconds | `2` | at least 1, less than `period_secs` | |
| `initial_delay_secs` | integer, seconds | `0` | 0–3600 | Wait before the first attempt |
| `failure_threshold` | integer | `3` | 1–100 | Consecutive failures before failing |
| `success_threshold` | integer | `1` | 1–100 for readiness; must be `1` for startup and liveness | Consecutive successes before passing |

## Probe

Set `type` to choose the probe.

### `http`

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `scheme` | `http` or `https` | `http` | Must be `https` when a header has a `secret` and `host` is not the workload. See [Where a vault value may go](#where-a-vault-value-may-go) |
| `host` | string | empty (the workload) | A host name or an IP address in normal form (numeric spellings like `2852039166` or `0xa9fea9fe` are refused). Any value other than the workload, `localhost` or a loopback address needs `allow_remote_host` |
| `allow_remote_host` | boolean | `false` | Required to probe another host. Metadata, link-local and IPv6 transition addresses and fleet nodes are refused anyway. See [Target rules](#target-rules) |
| `port` | integer | required | 1–65535 |
| `path` | string | `/` | Starts with `/`; at most 2048 characters |
| `method` | `GET` or `HEAD` | `GET` | |
| `headers` | list of [headers](#header) | empty | At most 20 |
| `expected_statuses` | list of `{from, to}` | `[{from: 200, to: 299}]` | 1–10 ranges within 100–599 |
| `tls_verify` | boolean | `true` | HTTPS: verify the certificate and host name. Must be `true` when a header has a `secret` and `host` is not the workload |
| `server_name` | string | empty | HTTPS: name to verify and send as SNI |

### `tcp`

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `host` | string | empty (the workload) | Same rules as HTTP |
| `allow_remote_host` | boolean | `false` | |
| `port` | integer | required | 1–65535 |

### `exec`

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `argv` | list of strings | required | The program, then its arguments: 1–64 items, each at most 4096 bytes. Started directly, never passed to a shell. On a native service, a relative program such as `./check.sh` starts from the service's folder |
| `expected_exit_codes` | list of integers | `[0]` | 1–16 codes, 0–255 |
| `show_output` | boolean | `false` | Report the end of a failed run's output as the check's detail. Left out of the stored policy when `false`. See [Output of failed runs](#output-of-failed-runs) |

### `shell`

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `interpreter` | `sh` or `bash` | `sh` | Native: found on the check's `PATH`. Container: must exist in the image |
| `script` | string | required | 1–16384 bytes. Native: the interpreter reads it from standard input. Container: passed as `<interpreter> -c <script>`, so it is visible in the process list of the container and of the node |
| `expected_exit_codes` | list of integers | `[0]` | 1–16 codes, 0–255 |
| `show_output` | boolean | `false` | As for `exec` |

Exec and shell checks (command checks) run as the service's own account,
or inside its container, on Linux nodes. Saving one, with or without
`show_output`, needs `health.command` (MFA-verified session). See
[How command checks run](#how-command-checks-run).

### Output of failed runs

| `show_output` | Failed run reports | Passing run reports |
| --- | --- | --- |
| `false` (default) | `exit code <n>`. No output leaves the node's helper | Nothing |
| `true` | `exit code <n>: <end of output>`: the last 160 bytes of the output, printable ASCII only. Stored with the result and shown in the console | Nothing |

In the console, the field is **Show the output of failed runs** in the
check's **Advanced** section.

:::caution
A command can print secrets. With `show_output: true`, the end of a failed
run's output is stored and shown to everyone with `health.view`. Leave it
off unless you know the output is safe.
:::

### Header

Set exactly one of `value` and `secret`.

| Field | Type | Notes |
| --- | --- | --- |
| `name` | string | An HTTP token, at most 64 characters |
| `value` | string | At most 1024 characters. Refused for credential headers: `Authorization`, `Proxy-Authorization`, `Cookie`, and names containing `token`, `secret`, `key`, `password` or `auth` |
| `secret` | string | A vault reference, `vault:<id>`. The item's password is sent as the whole header value. Saving it needs the right to reveal that item (platform administrator, `secret.read` with MFA, not with an API key); the item must exist. To a host other than the workload, only with `scheme: https` and `tls_verify: true`. See [Vault header values](#vault-header-values) |

## Example

```yaml
startup:
  - name: init
    probe: { type: http, port: 8080, path: /health/startup }
    period_secs: 5
    failure_threshold: 60
readiness:
  - name: api
    success_threshold: 2
    probe:
      type: http
      port: 8080
      path: /health/ready
      headers:
        - { name: Accept, value: application/json }
        - { name: Authorization, secret: "vault:<item-id>" }
liveness:
  - name: beat
    period_secs: 15
    timeout_secs: 3
    probe:
      type: shell
      script: |
        test -r /run/myapp/heartbeat || exit 1
```

The `api` check probes the workload itself, so its vault header may travel
over plain `http`. A check that sends a vault header to another host needs
`scheme: https`, with `tls_verify` left at `true`:

```yaml
readiness:
  - name: upstream
    probe:
      type: http
      scheme: https
      host: <upstream-host>
      allow_remote_host: true
      port: 443
      path: /health
      headers:
        - { name: Authorization, secret: "vault:<item-id>" }
```

On a native service, a command check reads only beneath the folders
listed in [How command checks run](#how-command-checks-run). `/run` is not
one of them, so a script like this `beat` check suits a container. For a
native service, keep the file in the service's folder:

```yaml
liveness:
  - name: beat
    period_secs: 15
    timeout_secs: 3
    probe:
      type: exec
      argv: ["test", "-r", "./heartbeat"]
```

To see the end of a failed run's output as the check's detail, add
`show_output: true` to the probe. Do so only when the output holds no
secrets:

```yaml
liveness:
  - name: queue
    period_secs: 30
    timeout_secs: 5
    probe:
      type: exec
      argv: ["./bin/queue-check"]
      show_output: true
```

## Target rules

These apply to the `host` of `http` and `tcp` probes. Forge applies them
when you save; the node applies them again to every address the host
resolves to, each time the check runs. One refused address refuses the
whole target.

| Target | When saved | On the node |
| --- | --- | --- |
| Not the workload, without `allow_remote_host` | Refused | failing, `invalid_config` |
| Cloud metadata and link-local: `169.254.0.0/16`, `fe80::/10`, `fd00:ec2::254`, in any encoding | Refused | failing, `invalid_config` |
| IPv6 transition addresses, whatever IPv4 address they name: 6to4 `2002::/16`, Teredo `2001::/32`, local-use NAT64 `64:ff9b:1::/48`, IPv4-translated `::ffff:0:0:0/96` | Refused: "IPv6 transition addresses (6to4, Teredo, NAT64) can't be probed; use the IPv4 address" | failing, `invalid_config`: "`<ip>` is an IPv6 transition address (6to4, Teredo or NAT64)" |
| Another node of the fleet | Refused for a node's host name, `.local` name or fabric address | failing, `invalid_config`: "`<ip>` is another node of the fleet" |

`2002:a9fe:a9fe::1`, for example, is one way to write `169.254.169.254`.

**The fleet list on a node.** When a policy probes a remote host, Forge
sends the addresses of the fleet's other nodes with it.

| Property | Value |
| --- | --- |
| Contents | For the nodes of the service's workspace only: where each agent connects from, the addresses in each node's facts (its interfaces included) and fabric addresses. The receiving node's own addresses are left out |
| Size | At most 2048 addresses |
| Sent | With every policy push: when you save the policy, when Forge sends the policy again after a revision mismatch, and every 10 minutes while the node reports |
| Staleness | At most about 10 minutes while the node is connected. A node added to the fleet later is not in a node's list until the next push. The rule at save time applies to it from the start. Saving the policy refreshes the list at once |
| When the list can't be built | Forge does not send the policy. It tries again on the node's next report |

## Policy delivery

| Event | What Forge does |
| --- | --- |
| You save the policy | Sends it at once to every node the service is placed on, with the signed app command `health` |
| A node reports results under another policy revision | Sends the policy again, at most once every 10 minutes per node and service |
| A node reports, and the policy has an `exec` or `shell` check or a probe with a remote `host` | Sends the policy again every 10 minutes |

On the node:

| Rule | Value |
| --- | --- |
| A policy that adds or removes command checks | Applied only when the node's helper took it. Otherwise the node keeps its previous policy and Forge sends the new one again |
| A signed policy issued before the one the helper holds | Refused by the helper (a delayed or replayed command) |
| A policy the helper has not received for 30 minutes | Its command checks do not run: `unknown`, `unsupported_executor`. They run again when the policy arrives |

## How command checks run

The node's root helper, `forge-agent-helper`, runs `exec` and `shell`
checks. It runs only checks from a policy Forge signed, which it keeps in
`/var/lib/forge-agent-helper/health/`; the agent names a check by
service, purpose, name and policy revision, and can't send a command.
For the reasoning, see
[Health checks](/docs/concepts/health-checks/#how-command-checks-run).

| | Service (native) | Container |
| --- | --- | --- |
| Supported nodes | Linux that enforces Landlock ABI 3 or later (kernel 6.2 or later, Landlock enabled) and has cgroup v2 with the `pids` and `memory` controllers | Linux with Docker |
| Started by | [`forge-agent-helper health-worker`](/docs/reference/cli/forge-agent-helper/#forge-agent-helper-health-worker) | Docker Engine API exec, in `forge-app-<environment>-<app>` only, and only when that container carries the label `forge.app` = `<environment>/<app>` |
| Runs as | The service's uid and gid, no supplementary groups | The container's default user. Not privileged, no TTY, no stdin |
| Contained in | A cgroup of its own, `/sys/fs/cgroup/forge-health/<environment>--<app>--<purpose>--<check>`, created before anything starts as the service's account. The worker is moved into it before it gets the command | The container |
| Working directory | `/srv/forge/apps/<environment>/<app>` | The container's default |
| Environment | `PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin`, `HOME=<app folder>`, `USER` and `LOGNAME` (the service's account), `LANG=C.UTF-8`. Nothing else: none of the service's variables or secrets | The container's own environment. Forge adds nothing |
| `exec` | `argv`, started directly | `argv`, as given |
| `shell` | The interpreter with no arguments; the script on standard input, so it is not in the node's process list | `<interpreter> -c <script>`: the script is one argument, visible in the process list of the container and of the node |
| Read and execute | Beneath `/usr`, `/bin`, `/sbin`, `/lib`, `/lib32`, `/lib64`, `/etc`, `/opt`, `/proc`, `/sys` and the service's folder, as far as the account's file permissions allow | Whatever the container allows |
| Read | `/dev/zero`, `/dev/random`, `/dev/urandom` | |
| Write | `/dev/null` only | Whatever the container allows |
| Privileges | `no_new_privs`: setuid programs gain nothing | |
| Network | TCP is not restricted | The container's network |
| Processes and memory | `pids.max` 64: processes and threads, the worker included. `memory.max` 256 MiB, no swap; where the kernel supports it, an out-of-memory kill takes the whole run | The container's limits |
| Resource limits | No core dumps, 256 open files, CPU time of `timeout_secs` + 5 s, files of at most 16 MiB | The container's limits |
| When the run ends or `timeout_secs` passes | The kernel kills everything in the cgroup (`cgroup.kill`) and the cgroup is removed. A new session, a new process group or a daemon (`setsid`, `nohup`, a double fork) does not leave it | The process Docker started is killed, after Forge verified it belongs to that container. Children it started in the container may survive. If it can't be ended, the check isn't started again and reports `timeout` until it ends |
| When the agent disconnects during a run | The same: the cgroup is killed | The same: the started process is ended |
| A command that exits but leaves a process holding its output open | Has not finished: it runs into the timeout | |

Windows nodes and jobs are not supported: the check reports `unknown`.

**What a native command check can and cannot do**

| Allowed | What |
| --- | --- |
| Cannot | Create, write, append to, truncate, rename or delete any file, also not in the service's folder or `/tmp` |
| Cannot | Gain privileges through setuid programs |
| Cannot | Outlive its run |
| Cannot | Have more than 64 processes and threads, or use more than 256 MiB of memory |
| Can | Read and execute the files beneath the folders above that the service's account may read |
| Can | Write to `/dev/null` |
| Can | Change the mode and timestamps of files the service's account owns. Landlock does not cover those |
| Can | Connect to UNIX sockets by path |
| Can | Use the network |

**Landlock by kernel**

| Landlock ABI | Effect |
| --- | --- |
| Below 3, or Landlock off | Native command checks do not run: `unknown`, `unsupported_executor` |
| 3 or 4 (Linux 6.2 or later) | The file rules above, truncation included |
| 5 | Also: device ioctls are controlled |
| 6 or later | Also: signals and abstract UNIX sockets stop at the check's own processes |

**Limits per node**

| Limit | Value |
| --- | --- |
| Command checks running at once on a node | 4, within the agent's 8 checks at once |
| Command checks of one service running at once | 2 |
| Runs of one check at once | 1 |
| Shortest time between two starts of one check | 0.9 s, kept by the helper |
| Wait for a free place | Up to 2 s. Then the check is turned away for this period: `unknown`, `too many command checks are running on this node` |
| Periods in a row without a place | The third and each one after it count as a failed attempt: `timeout`, `not run for <n> periods in a row: too many command checks are running on this node`. The check turns `failing` by its own `failure_threshold`. One run that gets a place starts the count over |
| Output kept from a failed run | None by default. With `show_output: true`, the last 160 bytes, printable ASCII only; other bytes become one space. Part of the 256-byte detail |
| Output kept from a passing run | None |

**Results**

| What happened | State | Reason | Detail |
| --- | --- | --- | --- |
| Exit code in `expected_exit_codes` | passing | | |
| Another exit code | failing | `app_failure` | `exit code <n>`. With `show_output: true` and output: `exit code <n>: <end of output>` |
| Ended by a signal | failing | `app_failure` | `ended by signal <n>` |
| Not finished within `timeout_secs` | failing | `timeout` | `not finished within <n> s; ended` |
| The previous run has not ended (container) | failing | `timeout` | `the previous run of this check has not ended` |
| No free place for 3 or more periods in a row | failing | `timeout` | `not run for <n> periods in a row: too many command checks are running on this node` |
| Program not found or not executable | failing | `invalid_config` | Why it could not start |
| Container not running | failing | `app_failure` | `the container is not running` |
| The node can't run the check | unknown (a startup check: `waiting` unless the refusal is permanent) | `unsupported_executor` | One of the sentences below |

`unknown` doesn't count toward `failure_threshold`.

**`unsupported_executor` details**

"Permanent" means the node keeps refusing until someone changes the node or
the service. The others pass on their own.

| Detail | Permanent | Meaning |
| --- | --- | --- |
| `this node can't confine command checks (its kernel does not enforce Landlock ABI 3 or later)` | Yes | Native service on a kernel older than 6.2, for example RHEL 8, or with Landlock off |
| `this node can't contain command checks (no cgroup v2 with the pids and memory controllers)` | Yes | Native service on a node with cgroup v1 or a hybrid layout, or without the `pids` or `memory` controller |
| `command checks need a Linux node and a service or container service` | Yes | A Windows node, or a job |
| `the service's own account or folder is missing on this node; deploy it again` | Yes | Native service without its account or folder on the node |
| `the container of that name on this node is not Forge's for this service` | Yes | Container service; the container `forge-app-<environment>-<app>` does not carry the label `forge.app` = `<environment>/<app>`. Nothing was run in it |
| `this node's helper has no signed policy for the service yet` | No | Forge sends the policy again within 10 minutes; saving sends it at once |
| `this node's helper holds another policy revision; waiting for the current one` | No | As above |
| `this node's helper has not received the policy from Forge for 30 minutes; waiting for it` | No | The node lost contact with Forge for that long. The check runs again when the policy arrives |
| `the signed policy on this node has no such command check` | No | The helper's policy has no enabled command check of that name and purpose |
| `the confined check worker could not start on this node` | No | The `health-worker` process did not start, refused to run or gave no valid answer |
| `Docker is not reachable on this node` | No | Container service; the Docker daemon did not answer |
| `too many command checks are running on this node` | No | No place became free within 2 s: 4 were running on the node, or 2 of the service. The check runs again next period |
| `the node's helper did not run the check (not running, or older than its agent): <what the helper said>` | No | The helper is not running, did not answer in time, or is older than the agent and does not know command checks. Update the node, or re-run the node installer |
| `the node's helper sent an answer this agent can't read` | No | Agent and helper are of different versions |

**Startup gate**

| Startup check | Its state | Readiness and liveness |
| --- | --- | --- |
| Refused permanently | `unknown` | Run |
| Refused for a cause that passes on its own, a vault value Forge refused or did not answer for included | `waiting`, with the reason and detail | `pending` |

## Vault header values

How a `secret: vault:<id>` header gets its value. The value is never in the
policy sent to the node and never on the node's disk.

| Property | Value |
| --- | --- |
| Value sent | The vault item's password, as the whole header value. For `Authorization`, store for example `Bearer <token>` |
| Fetched | By the node's agent, over its mTLS link to Forge, just before a probe that needs it, for one check at a time |
| Lifetime of a granted value | 300 seconds, in the agent's memory only. The agent never keeps one longer than 600 seconds. It drops the value when the lifetime ends, whether or not the check runs again, and when the check is disabled or removed |
| Lifetime of a refusal | 60 seconds |
| Unanswered request repeated after | 15 seconds |
| Requests per node | At most 120 per minute. Beyond that Forge refuses the request |
| Acting person | Whoever saved the policy's current revision. Forge records the person and the revision in the same transaction as the policy. The revision includes the service's listening port |
| Audit | `service.health.secret_use`, actor `agent:<node id>`, on the service, with the check, the policy revision, the vault item ids and `on_behalf_of`. Written for the first answer for a check on a node, on every change of the answer, and once per day while nothing changes. Never per probe; never a value. A value is not handed out when the entry that is due can't be written |
| Requests that name no real check on the node | Refused without an audit entry. Forge counts them and logs the count at most once a minute |

Forge grants the values only when all of these hold at the time of the
request:

| Condition | Otherwise |
| --- | --- |
| The node asked at most 120 times in this minute | `location_unavailable`: `Forge could not hand out the vault value`. Recovers by itself |
| The service is placed on the asking node | `invalid_config`: `Forge has no such check for a service on this node` |
| The node runs the stored policy revision | `invalid_config`: `this node runs another policy revision than Forge holds; waiting for the current one` (transient) |
| The stored policy has that check, enabled, with that reference | `invalid_config`: `Forge has no such check for a service on this node` |
| Someone saved the stored policy's current revision. A policy that changed another way, for example through a new listening port, has no saver until it is saved again | `permission_denied`: `Forge refused the vault value: the person who last saved this policy may no longer use it; save the policy again` |
| That person exists, isn't disabled, is a platform administrator and passes `secret.read` on the workspace with their current roles | `permission_denied`, with the same sentence |
| Every referenced item exists in the service's workspace | `invalid_config`: `a vault item this check refers to no longer exists` |
| The audit entry due for this answer was written | `location_unavailable`: `Forge could not hand out the vault value` |

When Forge doesn't answer (the control plane is unreachable, or older than
the node's agent), the reason is `location_unavailable` with
`Forge did not answer the request for the vault value`. When Forge fails
while it reads a value: `location_unavailable`,
`Forge could not hand out the vault value`.

In all these cases the check's state is `unknown` (a startup check:
`waiting`) and no request goes to the target.

A save that fails changes nothing, neither the policy nor the recorded
saver, and is audited as a failed `service.health.update`.

### Where a vault value may go

| Target of the probe | Allowed with a `secret` header |
| --- | --- |
| The workload: no `host`, `localhost` or a loopback address | `http`, or `https` with `tls_verify` `true` or `false` |
| Any other host | Only `scheme: https` with `tls_verify: true` |

| Checked | Otherwise |
| --- | --- |
| When you save | Refused, with the error on `probe.scheme` (plain `http`) or `probe.tls_verify` (`https` without verification): `a vault secret is sent to another host only over HTTPS with certificate verification` |
| On the node, on the connection it made: the peer is a loopback address, or the probe is verified HTTPS | failing, `invalid_config`, with the same sentence. Nothing is sent |

## Reasons in the console

| `reason` | Console label |
| --- | --- |
| `app_failure` | App answered with a failure |
| `timeout` | Timed out |
| `connect_failed` | Connection failed |
| `tls` | TLS handshake failed |
| `auth` | Authentication refused |
| `invalid_config` | Not allowed or not valid |
| `unsupported_executor` | Can't run on this node |
| `permission_denied` | Vault value refused |
| `location_unavailable` | Forge did not answer |
| `ambiguous_port` | Listening port is ambiguous |
| `node_offline` | Node offline |
| `stale` | Result is stale |

A check that every node refuses to run shows the pill **can't run**.

## Agent versions

| Node or server | Effect |
| --- | --- |
| Agent older than command checks and vault headers | Those checks report `unknown` with `unsupported_executor`. Update the node; agent and helper are updated together |
| Current agent, Forge server older than vault headers | Checks with a vault header report `unknown` with `location_unavailable` |

## Related

- [Health checks](/docs/concepts/health-checks/)
- [REST API: Service health checks](/docs/reference/rest-api/#service-health-checks)
