---
title: forge jobs
description: List, inspect, wait for, follow and cancel Forge's durable jobs from the command line.
sidebar:
  order: 6
---

`forge jobs` reads and controls the durable jobs that run every long or
remote operation, such as a stack deploy or a command on a node. For how jobs
work, read [Durable jobs](/docs/concepts/jobs/). The commands use the
[job endpoints](/docs/reference/rest-api/#jobs). Every command takes the
[global flags](/docs/reference/cli/#global-flags).

`<job>` is a job id. Commands that start jobs, such as
[`forge stacks deploy`](/docs/reference/cli/stacks/#forge-stacks-deploy), print
the ids. A job's `status` is `queued`, `running`, `succeeded`, `failed` or
`cancelled`; the last three are final.

## Permissions

| Commands | Cedar action | Scope | MFA |
| --- | --- | --- | --- |
| `list`, `get`, `steps`, `wait`, `watch` | `view` | the job | no |
| `cancel` | `job.cancel` | the job | no |

`job.cancel` is in the `platform_admin`, `workspace_admin` and `operator`
roles. See the [Cedar schema](/docs/reference/cedar-schema/).

## Commands

| Command | What it does |
| --- | --- |
| [`forge jobs list`](#forge-jobs-list) | Lists recent jobs, newest first |
| [`forge jobs get`](#forge-jobs-get) | Shows one job |
| [`forge jobs steps`](#forge-jobs-steps) | Lists a job's steps with status and error |
| [`forge jobs wait`](#forge-jobs-wait) | Waits until a job finishes, then prints it |
| [`forge jobs watch`](#forge-jobs-watch) | Follows a job's steps and live output until it finishes |
| [`forge jobs cancel`](#forge-jobs-cancel) | Asks a queued or running job to stop |

### forge jobs list

Lists recent jobs you can see, newest first. Needs `view` on each job; jobs
you cannot see are left out.

**Synopsis**

```text
forge jobs list [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--status <STATUS>` | enum: `queued`, `running`, `succeeded`, `failed`, `cancelled` | all | no | Only jobs with this status | `--status failed` |
| `--workspace <WORKSPACE>` | workspace id | none | no | Only this workspace's jobs | `--workspace <workspace-id>` |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

The server applies its limit before it leaves out jobs you cannot see, so
a page can have fewer rows than `--limit`. With `--all` the CLI asks for at
most 10,000 jobs.

**Examples**

```sh
# The 50 most recent jobs
forge jobs list
# The last 10 failed jobs
forge jobs list --status failed --limit 10
# Ids and errors of failed jobs, for a script
forge -o json jobs list --status failed | jq -r '.result[] | "\(.id) \(.last_error)"'
```

**Output**

```text
ID                                    KIND           STATUS     STEP  ERROR
01900000-0000-7000-8000-000000000008  agent.command  succeeded  2     -
01900000-0000-7000-8000-000000000007  agent.command  succeeded  2     -
01900000-0000-7000-8000-000000000009  stack.action   succeeded  3     -
01900000-0000-7000-8000-000000000010  stack.action   succeeded  3     -
01900000-0000-7000-8000-000000000011  stack.action   succeeded  3     -
```

`STEP` is the index of the next step to run. With `-o json`, `result` is an
array of job objects; see [`forge jobs get`](#forge-jobs-get) for the
fields.

```json
{
  "result": [
    {
      "attempts": 0,
      "context": {
        "mfa": true
      },
      "created_at": 1794590525506,
      "current_step": 2,
      "id": "01900000-0000-7000-8000-000000000008",
      "kind": "agent.command",
      "last_error": null,
      "status": "succeeded",
      …
    },
    …
  ],
  "schema": "forge.cli/v1"
}
```

Exit codes: `0` success; `2` an unknown `--status`; `3` not signed in.

**See also**: [`forge jobs get`](#forge-jobs-get),
[Durable jobs](/docs/concepts/jobs/).

### forge jobs get

Shows one job: its kind, payload, status, step and state. `forge jobs show`
is an alias. Needs `view` on the job.

**Synopsis**

```text
forge jobs get [OPTIONS] <JOB>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<job>` | job id | — | yes | The job | `01900000-0000-7000-8000-000000000008` |

**Examples**

```sh
# One job
forge jobs get <job-id>
# Only its status
forge -o json jobs get <job-id> | jq -r .result.status
```

**Output**

The table output prints the job as a key/value list. With `-o json`:

```json
{
  "result": {
    "attempts": 0,
    "context": {
      "mfa": true
    },
    "created_at": 1794590525506,
    "current_step": 2,
    "id": "01900000-0000-7000-8000-000000000008",
    "idempotency_key": null,
    "kind": "agent.command",
    "last_error": null,
    "lease_expires_at": null,
    "lease_owner": null,
    "lease_token": 1,
    "max_attempts": 3,
    "next_run_at": 1794590525506,
    "payload": {
      "command": {
        "type": "ping"
      },
      "server_id": "01900000-0000-7000-8000-000000000004"
    },
    "principal": {
      "id": "01900000-0000-7000-8000-000000000003",
      "type": "user"
    },
    "resource": {
      "id": "01900000-0000-7000-8000-000000000004",
      "kind": "server"
    },
    "state": {
      "operation_id": "685948c010b09b32c62c17bc3e06bab2",
      "output": {
        "pong": true,
        "uptime_secs": 650
      }
    },
    "status": "succeeded",
    "updated_at": 1794590525553,
    "workspace_id": "01900000-0000-7000-8000-000000000001"
  },
  "schema": "forge.cli/v1"
}
```

| Field | Description |
| --- | --- |
| `kind` | Job kind, such as `agent.command`, `stack.action`, `app.action` or `package.action` |
| `payload` | What the job was asked to do |
| `state` | Data carried between steps, saved after each step. `output` holds the node's result |
| `status` | `queued`, `running`, `succeeded`, `failed` or `cancelled` |
| `current_step` | Index of the next step to run |
| `attempts`, `max_attempts` | Tries so far and the most tries allowed |
| `context.mfa` | Whether the session that started the job was MFA-verified |
| `principal` | Who started the job |
| `resource` | What the job acts on |
| `last_error` | The last error, or `null` |
| `lease_owner`, `lease_expires_at`, `lease_token` | Which server process holds the job, until when, and its fencing token |
| `created_at`, `updated_at`, `next_run_at` | Epoch milliseconds |

Exit codes: `0` success; `3` not signed in; `4` no `view` on the job; `6` no
such job.

**See also**: [`forge jobs steps`](#forge-jobs-steps),
[`forge jobs watch`](#forge-jobs-watch).

### forge jobs steps

Lists a job's steps with their status and error. Needs `view` on the job.

**Synopsis**

```text
forge jobs steps [OPTIONS] <JOB>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<job>` | job id | — | yes | The job | `01900000-0000-7000-8000-000000000008` |

**Examples**

```sh
# Where did a job stop?
forge jobs steps <job-id>
```

**Output**

```text
#  STEP          STATUS     ERROR
0  dispatch      succeeded  -
1  await_result  succeeded  -
```

```json
{
  "result": [
    {
      "error": null,
      "finished_at": 1794590525536,
      "index": 0,
      "job_id": "01900000-0000-7000-8000-000000000008",
      "name": "dispatch",
      "started_at": 1794590525515,
      "status": "succeeded"
    },
    {
      "error": null,
      "finished_at": 1794590525552,
      "index": 1,
      "job_id": "01900000-0000-7000-8000-000000000008",
      "name": "await_result",
      "started_at": 1794590525539,
      "status": "succeeded"
    }
  ],
  "schema": "forge.cli/v1"
}
```

`started_at` and `finished_at` are epoch milliseconds; `finished_at` is
`null` while the step runs.

Exit codes: `0` success; `3` not signed in; `4` no `view` on the job; `6` no
such job.

**See also**: [`forge jobs get`](#forge-jobs-get).

### forge jobs wait

Waits until a job finishes, then prints it. The CLI polls the job, starting
every half second and backing off to every 5 seconds. Needs `view` on the
job.

**Synopsis**

```text
forge jobs wait [OPTIONS] <JOB>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<job>` | job id | — | yes | The job | `01900000-0000-7000-8000-000000000008` |
| `--timeout-secs <TIMEOUT_SECS>` | integer, seconds | `1800` | no | How long to wait before exit `8`. The job keeps running | `--timeout-secs 300` |

**Examples**

```sh
# Wait for a job, then print it
forge jobs wait <job-id>
# In a script: stop the pipeline when the job fails
forge jobs wait <job-id> --timeout-secs 600 || exit 1
```

**Output**

When the job succeeds, the job as in [`forge jobs get`](#forge-jobs-get):
a key/value list, or `{ "schema": "forge.cli/v1", "result": { … } }` with
`-o json`. When it fails, is cancelled or the wait times out, it prints the
error instead, with the code `job_failed`, `job_cancelled` or `timeout`.
Shape from the server's schema; not captured on a live system.

Exit codes: `0` the job succeeded; `3` not signed in; `4` no `view` on the
job; `6` no such job; `8` the wait timed out (the job keeps running); `10`
the job failed or was cancelled.

**See also**: [`forge jobs watch`](#forge-jobs-watch).

### forge jobs watch

Follows a job until it finishes. Each step change and the node's live
output go to stderr; the final job goes to stdout. With `-o json`, stderr
stays quiet. Needs `view` on the job.

**Synopsis**

```text
forge jobs watch [OPTIONS] <JOB>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<job>` | job id | — | yes | The job | `01900000-0000-7000-8000-000000000008` |
| `--timeout-secs <TIMEOUT_SECS>` | integer, seconds | `1800` | no | How long to follow before exit `8`. The job keeps running | `--timeout-secs 300` |

**Examples**

```sh
# Follow a job in the terminal
forge jobs watch <job-id>
# Follow a job and keep the result for a script
forge jobs watch <job-id> -o json > job.json
```

**Output**

On stderr, one line per finished step, followed by the node's output as it
arrives:

```text
  step 0 dispatch: succeeded
  step 1 await_result: succeeded
```

On stdout, the final job as in [`forge jobs get`](#forge-jobs-get). When
the job failed or was cancelled, the table output prints the job and then
the error on stderr. With `-o json`, stdout holds exactly one document: the
job when it succeeded, or only the [error document](/docs/reference/cli/#exit-codes)
(code `job_failed` or `job_cancelled`) when it did not. Shape from the CLI's
source; not captured on a live system.

Exit codes: `0` the job succeeded; `3` not signed in; `4` no `view` on the
job; `6` no such job; `8` the timeout ran out (the job keeps running); `10`
the job failed or was cancelled.

**See also**: [`forge jobs wait`](#forge-jobs-wait),
[`forge jobs cancel`](#forge-jobs-cancel).

### forge jobs cancel

Asks a queued or running job to stop. The running step stops at once, and
Forge tells the node to stop an operation it still runs for the job. It asks
first unless you pass `--yes`. Needs `job.cancel` on the job.

**Synopsis**

```text
forge jobs cancel [OPTIONS] <JOB>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<job>` | job id | — | yes | The job | `01900000-0000-7000-8000-000000000008` |
| `--yes` | flag | off | no | Skip the confirmation prompt | `--yes` |

:::caution
A cancelled job can leave its work half done, for example a package that is
partly installed. Check the node before you start the job again.
:::

**Examples**

```sh
# Cancel a job, then confirm its status
forge jobs cancel <job-id> --yes
forge jobs get <job-id>
```

**Output**

Nothing on success. With `-o json`, `result` is `null`. Shape from the
server's schema; not captured on a live system.

Exit codes: `0` the job is cancelled; `1` you did not answer `yes`; `2` a
change without `--yes` where no prompt is possible; `3` not signed in; `4`
no `job.cancel`; `6` no such job; `7` the job already finished.

**See also**: [`forge jobs get`](#forge-jobs-get),
[Durable jobs](/docs/concepts/jobs/).
