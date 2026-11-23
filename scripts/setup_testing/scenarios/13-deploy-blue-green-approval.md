# 13 · Blue-green with approval; pause, resume and abort

`hashicorp/http-echo` in `staging` behind endpoint `s13.lab.local`. A
blue-green release waits for someone other than its author to approve it,
then all traffic moves at once. A running rollout can be paused, resumed and
aborted.

**Run it:** `python 13-deploy-blue-green-approval.py [--only D04] [--keep]`

## Prerequisites

`10-deploy-setup.md`, including the approver (`..\.lab\deploy-approver.token`,
service account `deploy-approver` with `view` and `deploy.approve` on
service `lab`, which holds dev, staging and prod; a cluster grant would not reach services).

## 1. Release 1 and the endpoint

`s13-echo`: image `hashicorp/http-echo:1.0`, entrypoint
`/http-echo -text=v1`, ports `8081:5678`, recreate; endpoint `s13-echo` on
port 8081. Every answer is `v1`.

## D04 · Blue-green waiting for approval

```yaml
strategy: { type: blue-green, approval: { required: true, timeout: 10m } }
```

```powershell
forge deploy apply -f s13.deploy.yaml                 # returns the deployment id
forge deploy status <id>                               # awaiting_approval
forge deploy approve <id>                              # exit 4: the author cannot approve
$env:FORGE_TOKEN = Get-Content ..\.lab\deploy-approver.token
forge deploy approve <id>; Remove-Item Env:FORGE_TOKEN # approved
```

**Expected:** while it waits, every answer is still `v1` (the new copy runs
but has no traffic); after the approval every answer is `v2`, all at once.
The events name who approved.

## D14 · Pause, resume and abort

- Apply `v3` start-first with `stabilizationPeriod: 40s`, then `forge deploy
  pause <id>`: the state turns `paused` before the next step and the step
  count stops. `forge deploy resume <id>` finishes it.
- Apply `v4` with `stabilizationPeriod: 60s`, then `forge deploy abort <id>`
  during the gate: state `aborted`, the `v4` copy is removed, every answer is
  still `v3`.

## Clean up

Remove the endpoint, the service and its copies.
