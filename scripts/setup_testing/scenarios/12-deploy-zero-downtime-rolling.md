# 12 · Zero-downtime rolling updates behind an endpoint

A container service in `staging` behind an nginx endpoint on lab-a. A
start-first rollout runs the new release next to the old one, gates it,
moves the traffic and stops the old copy, while requests keep flowing.

**Run it:** `python 12-deploy-zero-downtime-rolling.py [--keep] [--reset]`

## Prerequisites

`10-deploy-setup.md`. nginx runs on lab-a.

## 1. First release in place, then the endpoint

Deploy `traefik/whoami:v1.10.1` as `s12-web` (recreate, `8080:80`, lab-c),
then create endpoint `s12-web` for `s12.lab.local` with target
`{kind: app, service_id: <s12-web>, port: 8080}` on edge lab-a, and apply it.

```powershell
curl.exe -s -H "Host: s12.lab.local" http://10.50.0.10/health
```

## D03 · Start-first under load

Keep a request loop running through the edge, then:

```powershell
# strategy: { type: rolling, replacement: start-first }, traffic: { drainTimeout: 10s }
forge deploy apply -f s12.deploy.yaml --wait          # → v1.11.0
forge deploy slots s12-web -n staging                  # slot a, weight 100
```

Then once more back to `v1.10.1`: the release lands in slot `b`.

**Expected each time:**
- exit 0 and **no failed request** in the loop;
- only the new slot serves (`a`, then `b`), the old copy is stopped
  (`docker ps` shows only `forge-app-staging-s12-web--a`, then `--b`);
- `/etc/nginx/forge/sites/s12-web.conf` on lab-a points at the slot's port
  (20000–29999), with no `weight=` left after the rollout.

## Clean up

`forge sites remove s12-web --yes; forge sites delete s12-web --yes`, then
remove and delete the service.
