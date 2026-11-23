# 11 · First release, in-place updates, rollback and the lock

One container service on lab-c in environment `dev`, no endpoint: what a
team sees on day one. Releases are numbered, the image tag is pinned to a
digest, a single-node service is not taken down by accident, and an earlier
release comes back with one command.

**Run it:** `python 11-deploy-first-release-and-rollback.py [--only D02] [--keep] [--reset]`

## Prerequisites

`10-deploy-setup.md` done (or `vm_lab.py reset --checkpoint deploy-ready`),
signed in.

## The document

```yaml
apiVersion: forge/v1
kind: Deployment
metadata: { name: s11-web, environment: dev, description: first release }
spec:
  runtime: container
  artifact: { image: traefik/whoami:v1.10.1 }
  template: { ports: ["8080:80"] }
  placement: { nodes: [lab-c] }
  strategy: { type: recreate }
  health:
    startupTimeout: 3m
    stabilizationPeriod: 5s
    checks:
      readiness: [{ name: http, probe: { type: http, port: 8080, path: /health } }]
```

## D01 · First release (recreate) ✅

```powershell
forge deploy apply -f s11.deploy.yaml --wait
forge deploy releases s11-web -n dev          # 1  traefik/whoami:v1.10.1@sha256:…
forge nodes exec lab-c --shell -- "docker ps --format '{{.Names}}'"   # forge-app-dev-s11-web
```

**Expected:** exit 0; service `s11-web` created in `dev`; release 1's
artifact carries the digest the node pulled; the single instance (no slot)
answers on `:8080`.

## D02 · Stop-first on one node

Change the image to `v1.11.0` and the strategy to
`{ type: rolling, replacement: stop-first }`.

```powershell
forge deploy plan -f s11.deploy.yaml          # refused: … set strategy.allowDowntime: true
```

Add `allowDowntime: true`, apply again.

**Expected:** the plan is refused with a message naming `allowDowntime`; with
it, release 2 goes live (downtime: yes in the plan).

## D08 · Rollback and redeploy

```powershell
forge deploy rollback s11-web -n dev --wait   # back to release 1
forge deploy history  s11-web -n dev          # newest: kind rollback, succeeded
forge deploy redeploy s11-web 2 -n dev --wait # release 2 again
forge deploy slots    s11-web -n dev          # current release 2
```

## D15 · One deployment at a time

Apply a slow release (`stabilizationPeriod: 90s`) without `--wait`, then apply
again at once.

**Expected:** the second apply exits `7` (conflict: "already has a deployment
running"); `forge deploy abort <id>` ends the first and frees the service.

## Verify

`forge deploy history s11-web -n dev` shows: apply 1 succeeded, apply 2
succeeded, rollback 1 succeeded, apply 2 succeeded, apply aborted.

## Clean up

`forge services remove s11-web --node lab-c`, then delete the service (the
script does both unless `--keep`).
