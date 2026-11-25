---
title: Durable jobs
description: Leases, fencing tokens, idempotency, step state, retries and crash recovery.
sidebar:
  order: 3
---

Every operation that changes infrastructure runs as a **job**: a database row
plus an ordered list of named steps defined by a job handler. Jobs survive
control-plane restarts without Redis or a message broker.

## Lifecycle

```diagram
alt: Job states
states:
  path:
    - { state: queued, edge: claim }
    - { state: running, edge: all steps ok, tone: accent }
    - { state: succeeded, tone: ok }
  transitions:
    - { from: running, to: queued, when: retry (backoff) }
    - { from: running, to: failed, when: "fatal / retries exhausted / denied", tone: danger }
    - { from: running, to: cancelled, when: cancel, tone: muted }
```

## How durability works

1. **Claim with a lease.** A worker atomically sets `status = running`, a
   `lease_expires_at`, and increments `lease_token` (the *fencing token*).
2. **Run one step.** While it runs, the worker renews the lease every
   `lease / 3`.
3. **Persist.** On success it writes the carried `state` and advances
   `current_step`, but only if its fencing token still matches.
4. **Crash.** If the process dies, the lease expires. The next worker claims
   the job with a new token and resumes at the first incomplete step. Writes
   from the old worker are rejected because its token is stale.

Because a step may run again after a crash, **steps must be idempotent**.

## Cancellation

Cancelling a queued or running job (`job.cancel`) marks it `cancelled`.
Steps already done stay done: Forge does not undo them. The running step is
stopped, and no further step runs. Cancelling a job that has already finished
returns `409`.

In the console, the **Jobs** drawer in the header lists running jobs first,
then the rest grouped by time (just now, earlier today, yesterday, older).
Each row shows what the job does and on which node, coloured by kind and
status, and running rows have a cancel action. The status filters show counts.

Read-only checks the console runs on its own, such as ingress status polls,
fact collection and container lists, are hidden by default. Select **Show
background checks** to see them; runs of the same check are folded into one
row (`×12`). A failed check always shows, marked **check failed** in amber
rather than as a failed action.

A job's detail shows its status, a timeline of its steps with durations, the
error of a failed step with a hint, a summary of known results (for example
ingress status), its console output, and the raw payload and result.

## Idempotency keys

`POST /api/v1/jobs` and server commands accept `idempotency_key`. Submitting
the same `(kind, idempotency_key)` twice returns the original job.

## Retries

A step returns `Retryable` or `Fatal`. Retryable failures re-queue the job with
exponential backoff and full jitter (base 1 s, max 5 min) until
`max_attempts`; the same step runs again.

## Authorization

Submission checks `job.create` plus the handler's action (for example
`server.manage` for read-only agent commands). The runner re-checks the
handler's action before every step with the authorization context captured at
submission (including MFA). A revoked role makes the next step fail with
`denied`, and the denial is audited.

## Built-in job kinds

| Kind | Steps | Action |
| --- | --- | --- |
| `agent.command` | `dispatch`, `await_result` | `server.manage` (read-only) / `server.command` (mutating) |
| `storage.action` | `dispatch`, `await_result` | `storage.manage` (`server.manage` for `status`) on the member node |
| `diagnostics.sleep` | `first`, `second`, `third` | `job.create` |

`diagnostics.sleep` exists to demonstrate and test crash recovery: kill the
server during step two and it continues after restart.

## Adding a job kind

Implement `forge_jobs::JobHandler` and register it in
`src/apps/forge-server/src/state.rs`. The engine does not change.

```rust
#[async_trait]
impl JobHandler for MyJob {
    fn kind(&self) -> &'static str { "proxy.apply" }
    fn steps(&self) -> &'static [&'static str] { &["validate", "stage", "apply", "verify"] }
    fn required_action(&self, _payload: &Value) -> Action { Action::ServerCommand }
    async fn run_step(&self, index: usize, ctx: &mut StepContext) -> Result<(), StepError> { /* … */ }
}
```
