---
title: Configure health checks
description: Add startup, readiness and liveness checks to a service in the console, as a form or as YAML.
sidebar:
  order: 30
---

This guide shows you how to add health checks to a service, test them,
and save them. For what the three kinds of check mean, see
[Health checks](/docs/concepts/health-checks/).

## Before you begin

- You need `health.view` to see checks and `health.configure` to change HTTP
  and TCP checks. Every role that can deploy has `health.configure`.
- Command and shell checks also need `health.command`. Where MFA is
  required, saving them asks you to verify with your code or passkey.
- A header that takes its value from the **Vault** needs the right to reveal
  that item: platform administrator and `secret.read`, in an MFA-verified
  session, not with an API key. A check that sends a vault value to a host
  other than the service must use HTTPS with certificate verification.
- The service must exist. HTTP and TCP checks run on every node the
  service is placed on, next to Forge's default checks.
- Command and shell checks run on Linux nodes only, for **Service** and
  **Container** services. They do not run on Windows nodes or for jobs.

  | Node | Service (native) | Container |
  | --- | --- | --- |
  | Linux with Landlock ABI 3 or later (kernel 6.2 or later, Landlock enabled) and cgroup v2 with the `pids` and `memory` controllers, for example Ubuntu 24.04 | Runs | Runs |
  | Linux with an older kernel or without Landlock, for example Debian 12 with its stock kernel (6.1), Ubuntu 22.04 (5.15) and RHEL 8 | Does not run | Runs |
  | Linux with cgroup v1 or a hybrid cgroup layout | Does not run | Runs |
  | Windows | Does not run | Does not run |

  To check a node, run these on it. The kernel must be 6.2 or later, the
  first list must contain `landlock`, and the second must contain `pids`
  and `memory`:

  ```sh
  uname -r
  cat /sys/kernel/security/lsm
  cat /sys/fs/cgroup/cgroup.controllers
  ```

  On a node without cgroup v2, the last file doesn't exist.
- The node's agent and helper must include command checks and vault headers.
  [Update the node](/docs/guides/update-agents/) if they are older; agent and
  helper are updated together.

## Steps

1. Open **Services**, select the service, and open the
   **Health Checks** tab.
2. The **Checks** panel draws the lifecycle as three stages, **Startup**,
   **Readiness** and **Liveness**, side by side (stacked on a narrow
   window). Each stage says what it does, lists its checks and ends with
   **On failure:**, what a failure causes with the configured period and
   failure threshold. Select **Add** in a stage, or **Add check** in the
   panel header and then the stage, and choose a preset: **HTTP endpoint**,
   **TCP port**, **Command** or **Shell script**. The new check's editor
   opens under the stages. Forge's built-in checks appear in dashed outline,
   tagged **Built-in** (**Replaced** once your readiness checks replace it);
   the **Default checks** switch under the panel header turns them off.
3. Fill in the check:
   - Give it a **Name** (lowercase letters, digits and dashes).
   - For HTTP, set the **Port** and **Path**. Leave **Host** empty to probe
     the service itself.
   - Set the timing: **Period**, **Timeout** (shorter than the period),
     **Failure threshold** and, for readiness, **Success threshold**.
   - For a command, give the program and its arguments. On a native
     service a relative path such as `./check.sh` starts from the
     service's folder. For a shell script, choose `sh` or `bash`; in a
     container, the image must contain that interpreter.
   - Open **Advanced** for headers, TLS options, a remote host and exit codes.
   - For a command or shell check, a failed run reports only its exit code.
     To also see the end of its output, turn on
     **Show the output of failed runs** under **Advanced**.
   - To send a credential, add a header with a **Vault** reference,
     `vault:<item-id>`. The item's password is sent as the whole header
     value: for an `Authorization` header, store `Bearer <token>` as the
     password. When the check has a **Host** other than the service,
     choose HTTPS and leave certificate verification on.
4. Read the summary at the top of the check's editor. It says in plain
   words what the check does and when it changes state. Problems show next
   to the field, and the editors of checks with problems stay open. Select
   a check in its stage to open or close its editor; its menu offers
   **Edit**, **Duplicate**, **Disable** and **Delete**.
5. To edit as YAML, switch to **YAML**. The form and YAML edit the same
   policy; comments and formatting are normalized when you switch back.
6. An **Unsaved changes** bar appears at the bottom. Select
   **Review and save**, check the list of changes, and confirm. Forge sends
   the saved checks to the service's nodes at once. You don't need to
   deploy again.

:::caution
A host other than the service needs **Allow probing this host**. Cloud
metadata and link-local addresses, IPv6 transition addresses (6to4, Teredo,
NAT64) and other nodes of the fleet are always refused.
:::

:::caution
A command or shell check runs code as the service's own account, or in
its container, every period. Review it like a deploy. In a container, a
shell check's script is visible in the process list of the container and of
the node, so don't put a credential in it.
:::

:::caution
A command can print secrets. With **Show the output of failed runs** on, the
end of a failed run's output is stored with the result and shown in the
console. Leave it off unless you know the output is safe.
:::

:::caution
A vault header sends the credential to the check's target on every attempt.
Use it only with a target you trust with that credential. To any host other
than the service itself, Forge sends it only over HTTPS with certificate
verification; it refuses to save, and the node refuses to run, anything
else. The check uses the right of the person who saved this revision of the
policy: when they lose it, or the policy changes another way, the check
stops until someone who may use the item saves again.
:::

:::tip
If the container already has a Docker `HEALTHCHECK`, the tab offers
**Import as liveness check**. It adds a shell check to your draft; review its
timeout, because Docker allows a timeout as long as the interval and Forge
doesn't.
:::

## Verify

- The status line at the top of the tab shows the overall state, how many
  nodes are healthy and the last result time, with one chip per node.
  Select a node's chip to see its failing checks and their last error. The
  service's **Overview** and **Nodes** table show the same status.
- Each check in a stage shows a result dot, and its open editor shows a
  result pill, for example **2/2 passing** or **1/2 passing**. It reads
  **no results yet** until the first report arrives, which takes up to one
  period plus one heartbeat (about 25 seconds). Select the dot or pill to
  see the state, the reason and the last result time on each node. A check that every node refuses to run shows
  **can't run**; the popover gives each node's sentence, for example "This
  node can't confine command checks (its kernel does not enforce
  Landlock ABI 3 or later)".
- A startup check that can't run yet reads **waiting** with the reason and
  the sentence, and readiness and liveness read **pending**. A startup check
  the node will never run as it is reads **unknown**, and readiness and
  liveness run.
- The footer of the **Checks** panel reads **Saved** with the time of the
  last change.
- `forge services health <service>` lists the same checks with their summaries.
- **Audit** shows an `service.health.update` entry with each check's
  name, type and target. For a check with a vault header, it also shows an
  `service.health.secret_use` entry per node, with the node as the
  actor and the person who saved the policy as `on_behalf_of`.

## Troubleshooting

### Saving

| Symptom | Cause | Fix |
| --- | --- | --- |
| **Review and save** is disabled with "You can view these checks but not change them" | You lack `health.configure` | Ask for a role that can deploy in the environment |
| "Command and shell checks need the health.command permission" | You lack `health.command`, or your session isn't MFA-verified | Verify with your code or passkey, or ask for the role |
| "the service changed since you opened it" | Someone saved the service after you opened the tab | Choose **Reload latest**, or keep your draft and save again |
| "another node of the fleet can't be probed from here" | The host is a node's name, `.local` name or fabric address | Probe the service on that node instead |
| "credentials must be a vault secret" | A credential header has a plain value | Store it in the **Vault** and use `vault:<id>` |
| "a vault secret is sent to another host only over HTTPS with certificate verification", on the scheme or on certificate verification | The check has a vault header and a **Host** other than the service, and it uses plain HTTP or has certificate verification off | Choose HTTPS and turn certificate verification on, or probe the service itself |
| "IPv6 transition addresses (6to4, Teredo, NAT64) can't be probed; use the IPv4 address" | The host is in `2002::/16`, `2001::/32`, `64:ff9b:1::/48` or `::ffff:0:0:0/96` | Use the IPv4 address the host stands for, if it is one you may probe |

### HTTP and TCP checks

| Symptom | Cause | Fix |
| --- | --- | --- |
| A check stays **no results yet** | The node is offline, or its agent is older than health checks | Bring the node online; [update the agent](/docs/guides/update-agents/). An offline node gets the checks when it reconnects |
| A check fails with "Not allowed or not valid" and a sentence about its target | The host resolves to an address the node may not probe: not the service itself without **Allow probing this host**, or a link-local or metadata address | Leave **Host** empty to probe the service, or allow the host on the check |
| A check fails, "Not allowed or not valid": "`<ip>` is an IPv6 transition address (6to4, Teredo or NAT64)" | The host name resolves to such an address on the node | Point the check at a name or address that resolves to a plain IPv4 or IPv6 address |
| A check fails, "Not allowed or not valid": "`<ip>` is another node of the fleet" | The host resolves to an address of another node | Probe the service on that node instead |
| A check passes although its target is a node that joined the fleet a few minutes ago | A node's list of fleet addresses is the one sent with the last policy. Forge sends it again every 10 minutes while the node is connected | Wait up to 10 minutes, or save the policy again to refresh the list at once |
| A check fails, "Not allowed or not valid": "a vault secret is sent to another host only over HTTPS with certificate verification" | The check has a vault header, and the address the node connected to isn't a loopback address while the probe isn't HTTPS with certificate verification. Nothing was sent | Use HTTPS with certificate verification on, or point the check at the service itself |
| An HTTP check fails with "status 301" or "status 302" | The endpoint redirects. Checks don't follow redirects | Point the check at the final path, or add the redirect status to the accepted statuses |
| An HTTPS check fails with "TLS handshake failed" | The certificate isn't trusted by the node or doesn't match the name | Set the **Server name** to the certificate's name, or turn off certificate verification for a self-signed certificate. A check that sends a vault header to another host can't turn it off: give that host a certificate the node trusts |

### Vault headers

A check that gets no vault value reads **unknown** and sends no request to
its target.

| Symptom | Cause | Fix |
| --- | --- | --- |
| "Vault value refused": "Forge refused the vault value: the person who last saved this policy may no longer use it; save the policy again" | The person who saved the policy was deleted or disabled, is no longer a platform administrator, or lost `secret.read`. Or nobody saved the policy's current revision: it changed another way, for example because the service's **Listening port** changed | Someone who may use the item saves the health policy again |
| "Not allowed or not valid": "a vault item this check refers to no longer exists" | The item was deleted from the **Vault** | Create the item again and put its new id in the header, or remove the header |
| "Not allowed or not valid": "this node runs another policy revision than Forge holds; waiting for the current one" | The node hasn't received the current policy yet | Wait: Forge sends it again within 10 minutes. Saving the policy sends it at once |
| "Not allowed or not valid": "Forge has no such check for a service on this node" | The stored policy no longer has this check with a vault header, or the service is no longer placed on the node | Wait for the node to receive the current policy, or save the policy again |
| "Forge did not answer": "Forge did not answer the request for the vault value" | The node can't reach the control plane, or Forge is older than the node's agent | Restore the connection, or upgrade Forge |
| "Forge did not answer": "Forge could not hand out the vault value" | The node asked for vault values more than 120 times in a minute, Forge could not write the audit entry for the answer, or Forge failed while it read the value | Wait: a rate limit clears by itself within a minute. If it stays, read the server log, then save the policy again |
| "Can't run on this node" on an HTTP check with a vault header | The node's agent is older than vault headers | [Update the node](/docs/guides/update-agents/) |

### Command and shell checks

A check a node can't run reads **unknown** with the reason "Can't run on
this node" and one of these sentences. It doesn't count as a failure. A
startup check reads **waiting** instead when the cause passes on its own.

| Symptom | Cause | Fix |
| --- | --- | --- |
| "this node can't confine command checks (its kernel does not enforce Landlock ABI 3 or later)" | The node's kernel is older than 6.2, has no Landlock, or has it turned off. A native command check never runs unconfined | Use a kernel 6.2 or later with `landlock` in `/sys/kernel/security/lsm`, or use an HTTP or TCP check |
| "this node can't contain command checks (no cgroup v2 with the pids and memory controllers)" | The node uses cgroup v1 or a hybrid layout, or its cgroup v2 lacks the `pids` or `memory` controller. A native command check runs only in a cgroup of its own | Boot the node with the unified cgroup hierarchy, or use an HTTP or TCP check |
| "command checks need a Linux node and a service or container service" | The node runs Windows, or the service is a job | Use an HTTP or TCP check |
| "the service's own account or folder is missing on this node; deploy it again" | The service was never deployed on this node, or its account or folder was removed | Deploy the service again |
| "the container of that name on this node is not Forge's for this service" | A container named `forge-app-<environment>-<app>` exists, but it doesn't carry Forge's label `forge.app` = `<environment>/<app>`. Nothing runs in it | Remove or rename the foreign container, then deploy the service again |
| "this node's helper has no signed policy for the service yet" | The helper hasn't received the policy yet | Wait: Forge sends it again within 10 minutes. Saving the policy sends it at once |
| "this node's helper holds another policy revision; waiting for the current one" | The agent and the helper hold different revisions, for example right after a deploy | Same as above |
| "this node's helper has not received the policy from Forge for 30 minutes; waiting for it" | The node lost contact with Forge for that long. The helper doesn't run command checks from a policy that old | Restore the node's connection to Forge. The check recovers when the policy arrives |
| "the signed policy on this node has no such command check" | The helper's copy of the policy doesn't contain this check | Save the policy again |
| "the confined check worker could not start on this node" | The helper could not start its `health-worker` process or got no valid answer from it. A run that used more than 256 MiB of memory can end this way: the kernel kills the whole run, the worker included | Read `journalctl -u forge-agent-helper` on the node; make the check use less memory; [update the node](/docs/guides/update-agents/) |
| "Docker is not reachable on this node" | The Docker daemon isn't running or doesn't answer | Start Docker on the node |
| "too many command checks are running on this node" | 4 command checks were already running on the node, or 2 of this service, and no place became free within 2 seconds | Nothing: the check runs again next period. Shorten slow checks or lengthen periods if it repeats |
| "the node's helper did not run the check (not running, or older than its agent): `<what the helper said>`" | The helper service isn't running, didn't answer in time, or is older than the agent and doesn't know command checks | Run `systemctl status forge-agent-helper` on the node. [Update the node](/docs/guides/update-agents/), or re-run the node installer if the service is missing |
| "the node's helper sent an answer this agent can't read" | The helper is newer or older than the agent | [Update the node](/docs/guides/update-agents/) |
| A command check reads "Can't run on this node" with another sentence | The node's agent is older than command checks | [Update the node](/docs/guides/update-agents/) |

A check that ran and failed reads **failing**:

| Symptom | Cause | Fix |
| --- | --- | --- |
| "exit code `<n>`" | The command ended with a code that isn't in the accepted exit codes | Fix what the check reports, or add the code under **Advanced**. To see why it failed, turn on **Show the output of failed runs** if the output is safe to show |
| "exit code `<n>`: `<output>`" | The same, on a check with **Show the output of failed runs** on. The text is the end of its output | Fix what the check reports, or add the code under **Advanced** |
| "ended by signal `<n>`" | A signal ended the command, for example the CPU time limit | Make the check cheaper |
| "not finished within `<n>` s; ended" | The command ran longer than **Timeout**. On a native service, also a command that exited but left a process holding its output open, for example a background job | Make the check faster, or raise **Timeout** and **Period**. Don't start background processes in a check |
| "not run for `<n>` periods in a row: too many command checks are running on this node" | The check found no free place for 3 or more periods in a row. From the third on, each miss counts as a failed attempt | Shorten slow command checks, lengthen periods, or use fewer command checks on the node |
| A command fails with "Resource temporarily unavailable" or can't fork | A native command check may have at most 64 processes and threads | Make the check start fewer processes |
| "the previous run of this check has not ended" | In a container, the timed-out process could not be ended. Forge doesn't start the check again until it ends | Find out why the command hangs in the container; restart the container to clear it |
| "Not allowed or not valid" with a sentence that the program can't be started, for example "cannot start `<program>`: ..." | The program doesn't exist, isn't executable for the service's account, or lies outside the folders a check may read | Use a program beneath the system folders or the service's folder. In a container, use one that exists in the image |
| "the container is not running" | The service's container is stopped | Start or redeploy the service |
| A script fails with "Permission denied" when it writes a file | A native command check can't create, write, truncate, rename or delete files anywhere, `/tmp` included. It can write only to `/dev/null` | Remove the write from the check |
| A process the check started in the background is gone | When the run ends or its timeout passes, the kernel kills everything the check started, also what it moved to a new session (`setsid`, `nohup`) | Don't start long-running work from a check |
| A script can't see the service's environment variables | A native command check gets a minimal environment, without the service's variables or secrets | Read what you need from a file in the service's folder, or ask the service over HTTP |

## Next steps

- [Health policy reference](/docs/reference/health-policy/): every field and limit.
- [How command checks run](/docs/concepts/health-checks/#how-command-checks-run)
  and [Vault values in headers](/docs/concepts/health-checks/#vault-values-in-headers).
- [Health observations](/docs/concepts/health-observations/): how results are recorded.
- [REST API: Service health checks](/docs/reference/rest-api/#service-health-checks)
