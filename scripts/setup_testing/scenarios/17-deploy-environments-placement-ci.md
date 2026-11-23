# 17 · Environments, placement and CI

One document for two environments, nodes chosen by tags, a CI job that may
deploy only its own service, and the console.

**Run it:** `python 17-deploy-environments-placement-ci.py [--only D19] [--keep]`

## Prerequisites

`10-deploy-setup.md` (tags: lab-b `zone:a runtime:native`, lab-c
`zone:b runtime:native docker:yes`).

## D17 · One document, two environments

`s17-echo` (http-echo on lab-c), `metadata.environment: staging`, entrypoint
`/http-echo -text=staging`, ports `8083:5678`, and:

```yaml
spec:
  overrides:
    prod:
      template: { ports: ["8084:5678"], entrypoint: /http-echo -text=prod }
```

```powershell
forge deploy apply -f s17.deploy.yaml --wait
forge deploy apply -f s17.deploy.yaml --environment prod --wait
```

**Expected:** two services `s17-echo`, one per environment; on lab-c `:8083`
answers `staging`, `:8084` answers `prod`.

## D18 · Placement by tags

`s17-py` (hello-py archive) with `placement: { nodeSelector: { runtime:
native }, replicas: 2 }` → lab-b and lab-c. Then `nodeSelector: { zone: a }`.

**Expected:** first on both nodes; then only lab-b, and the service is
removed from lab-c (its remove job runs after the rollout; no
`forge-app-dev-s17-py*` unit left there).

## D19 · CI with a restricted token

```powershell
forge service-accounts create ci-s17
forge access profiles create "CI deployer" --action view --action deploy --action job.create
forge access assignments add svc-ci-s17 "CI deployer" service:<s17-echo id> --yes
forge -o json service-accounts token-create ci-s17 run-1 --expires-days 1 `
  --allow "service:<s17-echo id>=view,deploy,job.create"
python labkit\ci_deploy.py --template s17.template.yaml --var TAG=ci-v2 --token-file ci.token
```

`s17.template.yaml` is the D17 document with `-text=${TAG}`.

**Expected:** the token lists its restriction; the CI deploy exits 0 and
`:8083` answers `ci-v2`; the same token on another service (`s17-py`) exits
`4`; it cannot approve (no `deploy.approve`).

## D20 · The console (manual)

In the console, **Services → s17-echo (staging) → Releases**:

- [ ] Current release and **Copies** (node, slot, release, traffic, state) match `forge deploy slots`.
- [ ] **Apply YAML**: paste the D17 document with `-text=console`, **Plan** shows the steps and downtime; **Apply** opens the deployment.
- [ ] The deployment drawer ticks steps as they finish and streams events; **Pause**/**Resume**/**Abort** work.
- [ ] A deployment waiting for approval shows **Approve**/**Reject** only to someone other than its author.
- [ ] **Roll back** (confirm) and **Roll out again** on an older release work.
- [ ] A failed request (stop the server briefly) shows an error, not an empty list.

## Clean up

Remove `s17-echo` (staging, prod), `s17-py`; revoke the `ci-s17` token.
