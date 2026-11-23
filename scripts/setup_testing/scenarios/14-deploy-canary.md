# 14 · Canary releases and their gates

`hashicorp/http-echo` in `prod` behind endpoint `s14.lab.local`. A canary
moves traffic in weighted steps. A canary of a release that answers 500 trips
its error-rate gate and the previous release comes back.

**Run it:** `python 14-deploy-canary.py [--only D06] [--keep]`

## Prerequisites

`10-deploy-setup.md`. The edge writes `us=` and `rt=` in
`/var/log/nginx/forge-traffic.log` (any endpoint applied by this build).

## 1. Release 1 and the endpoint

`s14-echo`: entrypoint `/http-echo -text=v1`, ports `8082:5678`, recreate;
endpoint `s14-echo` on port 8082.

## D05 · Canary 20 % → 50 % → 100 %

```yaml
strategy:
  type: canary
  steps:
    - { weight: 20, pause: 90s }
    - { weight: 50, pause: 30s }
    - { weight: 100 }
```

During the first pause, send 400 requests through the edge and count the
answers.

**Expected:** about 20 % `v2` (10–32 % accepted); `weight=20` and `weight=80`
in `/etc/nginx/forge/sites/s14-echo.conf`; at the end every answer is `v2`
and no `weight=` is left.

## D06 · A gate trips

Release with entrypoint `/http-echo -text=boom -status-code=500`, readiness
left to the default check (`health.checks: {}`, so the copy starts), and:

```yaml
strategy:
  type: canary
  steps:
    - { weight: 30, pause: 60s, analysis: { maxErrorRate: 5, minRequests: 20 } }
    - { weight: 100 }
```

Keep a request loop running so the canary gets traffic.

**Expected:** `--wait` exits `10`. The deployment is `rolled_back` with an
error like `canary at 30%: 28% of 140 requests failed (at most 5%)`. An
`auto_rollback` deployment of release 2 follows and succeeds; one copy
serves, every answer is `v2`.

## Clean up

Remove the endpoint, the service and its copies.
