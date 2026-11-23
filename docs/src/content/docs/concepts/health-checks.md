---
title: Health checks
description: What startup, readiness and liveness checks mean in Forge, where they run, and the safety rules that bound them.
sidebar:
  order: 15
---

Health checks tell Forge whether a service instance has started,
can take traffic, and is still working. Each check result becomes a
[health observation](/docs/concepts/health-observations/) that the console,
alerts and automation read.

## Three purposes

| Purpose | Question | Effect |
| --- | --- | --- |
| Startup | Has this instance finished starting? | Readiness and liveness wait until it succeeds. When its failure budget runs out, the instance reports `exhausted` |
| Readiness | Can it take traffic now? | Reports whether the instance is eligible for traffic. Routing changes only through an explicitly configured adapter |
| Liveness | Is it working rather than stuck? | Reports failure. Forge never restarts an instance because of it on its own |

A failed readiness check doesn't restart a working process. A dependency
outage can make an API unready while it stays live. Keep liveness checks
cheap and local, so a broken dependency doesn't cascade into restarts.

Process state, readiness and liveness are separate. A running process isn't
necessarily ready, and a node that lost its connection makes checks
`unknown`, not failed.

## Probe types

| Type | Succeeds when | Notes |
| --- | --- | --- |
| HTTP or HTTPS | The response status is in the accepted ranges (default 200–299) | `GET` or `HEAD`, custom headers, certificate verification on by default, optional SNI name |
| TCP | A connection opens | Says the port is open, not that the service works |
| Exec | The program exits with an accepted code (default 0) | A program and its arguments; never passed through a shell. Linux nodes only |
| Shell | The script exits with an accepted code | `sh` or `bash`, chosen explicitly. Linux nodes only |

Exec and shell checks are called command checks. See
[How command checks run](#how-command-checks-run).

## Where checks run

The node's agent runs the checks, next to the service, and reports the
results with its heartbeat. The control plane never connects to your
services to check them.

- Saving a policy sends it to every node the service is placed on. No
  deploy is needed. A node that was offline, or that was just redeployed,
  reports under an older policy revision; Forge then sends it the current
  policy again.
- While a node reports, Forge sends it the policy of a service that has
  command checks, or that probes a remote host, again every 10 minutes. The
  helper runs command checks only from a policy it received recently, and
  the [fleet list](#the-fleet-list) is refreshed with it.
- The agent runs HTTP, HTTPS and TCP checks itself. It hands command checks
  to the node's root helper, which runs them as the service's own
  account or inside its container.
- A policy that adds or removes command checks applies on a node only when
  the node's helper took it. Otherwise the node keeps its previous policy
  and Forge sends the new one again.
- A check a node can't run safely doesn't run there. It reports `unknown`
  with a reason and a sentence that says why. `unknown` is not a failure:
  it doesn't count toward the failure threshold.
- A startup check runs until its first success (`succeeded`) or until its
  failure threshold is reached (`exhausted`). Both are final until the
  workload restarts. While a startup check hasn't succeeded, readiness and
  liveness checks read `pending`. See [The startup gate](#the-startup-gate)
  for what happens when a startup check can't run.
- A node runs at most 8 checks at the same time. A check that finds no free
  slot runs a second later. At most 4 of them are command checks, at most 2
  per service, and a check never overlaps itself.

## The startup gate

Readiness and liveness wait for the startup checks. What a startup check
that can't run does to them depends on why it can't run:

| The node's refusal | Startup check reads | Readiness and liveness |
| --- | --- | --- |
| Permanent: the node won't run the check until someone changes the node or the service. Not a Linux node or a job, no Landlock ABI 3, no cgroup v2, the service's account missing, a container that isn't Forge's | `unknown` | Run |
| Passes on its own: the policy is on its way or expired, the node is busy, the helper doesn't answer, Forge doesn't answer or refuses a vault value | `waiting`, with the reason and the sentence | Stay `pending` |

Only a permanent refusal releases the gate. A startup check that would run a
moment later must not let traffic checks start early, so a refusal that
passes on its own keeps the gate closed.

## Default checks

Every service gets a check without anyone configuring one, so Forge always
knows whether it is up on each node:

| Runtime | Default liveness |
| --- | --- |
| Service (native) | The systemd unit is active (on a node without systemd, such as a container: Forge's supervised process is running), plus a TCP connect to the service's **Listening port** when one is set |
| Job (native) | The systemd unit is active |
| Container | The container is running, plus its image's Docker `HEALTHCHECK` if it has one, else a TCP connect to the published port |

Default readiness is the same as liveness until you configure a readiness
check, which then replaces it. Default checks run every 10 seconds and
report unhealthy after 3 failures. You can turn them off on the
**Health Checks** tab (the **Default checks** switch at the top of the **Checks**
panel, where they show in dashed outline in their stage); with no other checks, Forge
then can't tell whether the service is healthy.

## Status per node

Each node the service runs on gets one status from its checks:

| Status | When |
| --- | --- |
| Healthy | Startup succeeded (or there is no startup check), liveness passes and every required readiness check passes |
| Degraded | Everything required passes, but an optional readiness check fails |
| Unhealthy | Liveness fails, a required readiness check fails, or startup ran out of its failure budget |
| Unknown | A required check hasn't passed yet, its result is stale, or the node is offline |

When a node goes offline, its checks turn **Unknown** at once, with the
reason "node offline". Forge doesn't keep showing the last state until it
expires.

Nodes report results with their regular heartbeat, so a change can take up
to one heartbeat (about 25 seconds) to show, after the 3 failures that turn
a check unhealthy.

If two services on one node declare the same **Listening port**, that port
is ambiguous: the node checks only the systemd unit for them, and the console
shows a warning. Forge never wires anything to an ambiguous port
automatically.

## Timing

Every check has a period, a timeout shorter than the period, an initial
delay, a failure threshold and a success threshold. Thresholds count
completed consecutive results, not retries inside one attempt. Startup and
liveness checks turn passing after one success; readiness checks can ask for
more.

The console shows a plain-language summary of each check, built from the
same settings, for example: "Check GET http://the workload:8080/health/ready
every 10 s (timeout 2 s). Mark unready after 3 consecutive failures; ready
again after 2 successes."

Several checks of one purpose combine: an instance is ready when all
**required** readiness checks pass. An optional readiness check that fails
shows the instance as degraded.

## Safety rules

Checks run on your nodes, so Forge bounds what they can reach and run:

- **Targets.** HTTP and TCP checks probe the workload itself by default:
  `127.0.0.1` on the node, which is the service's own port for native
  apps and the port the container publishes on the node for containers.
  Any other host must be allowed explicitly on the check. Cloud metadata and
  link-local addresses (`169.254.0.0/16`, `fe80::/10`, `fd00:ec2::254`) are
  always refused, including encoded forms such as `2852039166`,
  `0xa9fea9fe` or `::ffff:169.254.169.254`. IPv6 transition addresses are
  refused too, whatever IPv4 address they name: 6to4 (`2002::/16`), Teredo
  (`2001::/32`), local-use NAT64 (`64:ff9b:1::/48`) and IPv4-translated
  (`::ffff:0:0:0/96`). `2002:a9fe:a9fe::1`, for example, is one way to
  write `169.254.169.254`. Forge applies these rules when you save and the
  node applies them again when the check runs. The node resolves the host
  once, checks every address it gets (one forbidden address refuses the
  whole target) and connects only to a checked address. Redirects are not
  followed: a `3xx` answer is a status like any other.
- **Other nodes.** A check may not probe another node of the fleet. When you
  save, Forge refuses node host names, `.local` names and fabric addresses.
  When a policy probes a remote host, Forge also sends the addresses of the
  fleet's other nodes with it, and the node refuses a target that resolves
  to one of them. See [The fleet list](#the-fleet-list) for its limit.
- **Who runs commands.** Exec and shell checks run only as the service's
  own account on native apps, confined with Landlock inside a cgroup of
  their own, or inside the service's own container. Never as `root`, and
  never as the agent's or the helper's user. Where a node can't guarantee
  that, the check doesn't run. Saving such a check needs `health.command`,
  which requires an MFA-verified session. Developers have it: they already
  deploy code that runs as that account.
- **Secrets.** Credential headers (`Authorization`, `Cookie`, names
  containing `token`, `secret`, `key`, `password` or `auth`) take a vault
  reference, `vault:<id>`. Plain values are refused. Using a vault item in a
  check needs the same right as revealing it (platform administrator,
  `secret.read` with MFA), and the item must exist. A vault value goes to a
  host other than the workload only over HTTPS with certificate
  verification. See [Vault values in headers](#vault-values-in-headers). The
  audit log records which vault items a check uses, but never their values,
  header values, scripts or query strings.
- **Output.** A command's output stays on the node unless the check asks
  for the output of failed runs (`show_output`).

:::caution
A command check runs real code as the service's account, or in its
container, every period. Treat it like a deploy: review the program or
script, and keep it fast and side-effect free.
:::

## How command checks run

The agent never runs a command check itself. The node's root helper,
`forge-agent-helper`, does, and it runs only what Forge signed.

The helper keeps the command checks of each service in its own
root-only folder, `/var/lib/forge-agent-helper/health/`. It takes them only
from commands Forge signed: the policy Forge sends, and a deploy. The agent
asks the helper to run a check by reference: the service, the purpose,
the check's name and the policy revision. It can't send a command, arguments
or a script. So someone who takes over the agent's user can start only
checks Forge authorized, and no faster than the helper allows.

A stored policy also has an age. The helper remembers when Forge issued the
policy it holds and refuses an older signed one, so a delayed or replayed
command can't bring back an earlier set of checks. Forge sends the policy
again every 10 minutes while the node reports. The helper doesn't run a
command check from a policy it hasn't received for 30 minutes: a node that
lost contact with Forge stops running commands Forge may since have
withdrawn. The checks report `unknown` until the policy arrives again.

Where the command runs depends on the service's runtime:

| | Service (native) | Container | Job, or a Windows node |
| --- | --- | --- | --- |
| Runs as | The service's own account, with no other groups | The container's default user, inside the service's own container | Doesn't run |
| Confinement | Landlock for files and privileges, and a cgroup of its own for processes and memory | The container's own isolation. Not privileged, no TTY, no stdin. The container must carry Forge's label for the service | |
| Environment | A fixed minimal one. None of the service's variables or secrets | The container's own environment | |
| Working directory | The service's folder | The container's default | |
| `shell` script | Read by the interpreter from standard input, so it isn't in the node's process list | One argument of `<interpreter> -c`, so it is visible in the process list of the container and of the node | |
| When the run ends or its timeout passes | The kernel kills everything in the run's cgroup | The process Docker started is ended; its children in the container may survive | |

### What a native check can and can't do

A native command check can't:

- Create, write, append to, truncate, rename or delete any file. Not in the
  service's folder and not in `/tmp`.
- Gain privileges through setuid programs.
- Outlive its run. Starting a new session, a new process group or a daemon
  (`setsid`, `nohup`, a double fork) doesn't take a process out of the
  cgroup, and the kernel kills the whole cgroup when the run ends or its
  timeout passes.
- Use more than 64 processes and threads, or more than 256 MiB of memory. A
  fork bomb stops at 64.

It can:

- Read and execute the files beneath the system folders and the
  service's folder that the service's account may read.
- Write to `/dev/null`.
- Change the mode and timestamps of files the service's account owns.
  Landlock doesn't cover those operations.
- Connect to UNIX sockets by path.
- Use the network. TCP isn't restricted: most checks ask the service
  over a connection.

On kernels that support it, the check also can't use device ioctls, signal
processes outside itself or reach abstract UNIX sockets.

There is no unconfined mode. A native command check runs only on a node
whose kernel enforces Landlock ABI 3 or later (Linux 6.2), the first that
controls truncation, and that has cgroup v2 with the `pids` and `memory`
controllers. Elsewhere (an older kernel such as RHEL 8's, or a cgroup v1 or
hybrid layout) native command checks don't run; they report `unknown`
with the reason `unsupported_executor`. HTTP and TCP checks work on every
node. The trade-off is deliberate: a check that silently ran with the
account's full rights, or that could leave processes behind, would be a
second, unreviewed way to change the service.

A command that exits but leaves a process holding its output open hasn't
finished. The run lasts until its timeout and counts as timed out.

### Containers

Before it runs anything, the helper checks that the container named
`forge-app-<environment>-<app>` carries Forge's label for that service.
The name alone can't tell two services apart whose names join to the
same string. In a container that isn't Forge's, nothing runs.

### Output

The helper reads a command's output to the end. By default none of it
leaves the helper: a failed run reports only its exit code. When the check
sets `show_output`, the end of a failed run's output, at most 160 bytes of
printable ASCII, becomes the check's detail. It is stored with the result
and shown in the console. A passing run never reports output.

:::caution
A command can print secrets. Leave `show_output` off unless you know the
output is safe to store and show. In a container, a shell check's script is
visible to every process that can list the processes of the container or of
the node, so don't put a credential in the script either.
:::

### Limits and queueing

A node runs at most 4 command checks at once, and at most 2 of one
service. A check waits up to 2 seconds for a free place. Then it is
turned away for this period and reports `unknown`. The third period in a row
without a place, and each one after it, counts as a failed attempt. So a
check that never gets to run turns `failing` by its own failure threshold
instead of staying unknown. One run that gets a place starts the count over.

For the exact folders, limits and results, see
[Health policy: How command checks run](/docs/reference/health-policy/#how-command-checks-run).

## Vault values in headers

A header with `secret: vault:<id>` is sent with the vault item's password as
the whole header value. The value is never in the policy Forge sends to the
node and never written to the node's disk.

Just before a probe that needs it, the node's agent asks Forge for the
values of that one check over its mutually authenticated link. Forge decides
each time, and hands the values out only when all of this holds:

- The node asks no more than 120 times per minute.
- The service is placed on the asking node, and the node runs the stored
  policy revision.
- The stored policy still has that check, enabled, with that reference.
- Someone saved exactly this revision of the policy.
- That person still exists, isn't disabled, is still a platform
  administrator and still passes `secret.read` on the workspace with their
  current roles.
- The item still exists in the service's workspace.
- The audit entry that is due for this answer was written.

The check borrows the right of the person who saved the policy, and only for
the revision they saved. Forge records who saved it together with that
revision, in the same transaction as the policy. Their MFA was verified at
that moment; the other conditions are checked again on every request.

The revision covers more than the checks: the service's
**Listening port** is part of it. When the policy changes another way, for
example because someone changes the listening port, nobody has saved the new
revision. The check then reports `unknown` with the reason
`permission_denied` until someone who may use the item saves the health
policy again. A person's authority never carries over to a policy they
didn't save.

Forge answers with the values and a lifetime of 300 seconds. The agent keeps
them in memory for that long, then asks again. So revoking the saver's role,
disabling them or deleting the item takes effect within 5 minutes. The agent
drops a value from its memory when its lifetime ends, whether or not the
check runs again, and when the check is disabled or removed. When Forge
refuses or doesn't answer, the check reports `unknown` and no request goes
to the target. The agent remembers a refusal for 60 seconds.

Forge audits the use as `service.health.secret_use`, with the node as
the actor and the saver as `on_behalf_of`. It writes an entry for the first
answer for a check on a node, when the answer changes, and once per day
while nothing changes. It never writes one per probe and never a value. When
an entry is due and can't be written, Forge doesn't hand out the value.
Requests that name no real check on the node are refused without an audit
entry, so a node can't fill the log; Forge counts them and logs the count at
most once a minute. Values don't appear in logs, reports, the audit log or
check details.

### Where a vault value may go

A vault value travels in clear text, or to a server nobody verified, only
when the target is the workload itself: no `host`, `localhost` or a loopback
address. For any other host the probe must use `https` with certificate
verification on. Forge refuses to save anything else.

The node checks again on the connection it made, because a name such as
`localhost` is whatever the node's resolver says. When the peer isn't a
loopback address and the probe isn't verified HTTPS, the check fails with
`invalid_config` and nothing is sent.

:::caution
A vault header sends the credential to the probe's target on every attempt.
Point such a check only at an endpoint you trust with that credential.
:::

## The fleet list

The list of fleet addresses a node checks targets against is the one Forge
sent with the last policy. It covers the nodes of the service's
workspace only: where each agent connects from, the addresses in each node's
facts (its interfaces included) and fabric addresses, up to 2048, without
the receiving node's own.

Forge sends a policy that probes a remote host again every 10 minutes while
the node reports, and builds the list anew each time. So a node added to the
fleet later is missing from a connected node's list for at most about 10
minutes. The check at save time applies to it from the start. Saving the
policy refreshes the list at once.

When Forge can't build the list, it doesn't send the policy: a node with a
list that is missing a source could not refuse those addresses. Forge tries
again on the node's next report.

## The container health check

A container can also have a Docker `HEALTHCHECK` (the service's
**Health check** field). Docker runs that one; Forge doesn't convert it. The
**Health Checks** tab shows it and offers to import it as a liveness check
you then own.

## Related

- [Health observations](/docs/concepts/health-observations/)
- [Health policy reference](/docs/reference/health-policy/)
- [REST API: Service health checks](/docs/reference/rest-api/#service-health-checks)
- [`forge services health`](/docs/reference/cli/services/#forge-services-health)
