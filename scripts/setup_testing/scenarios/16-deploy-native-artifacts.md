# 16 · Native services from fetched code

Services and jobs that run as their own Linux account from code the node
fetches: a release asset (`.tar.gz`, `.zip`) checked by SHA-256, or a pinned
Git commit — what a GitHub release or tag gives you.

**Run it:** `python 16-deploy-native-artifacts.py [--only D10] [--keep]`

## Prerequisites

`10-deploy-setup.md` (lab CA trusted, `python3`, `git`, `unzip` on lab-b and
lab-c). `hello-py` reads `$PORT` (default 8090); `/ready` answers 200.

## The document

```yaml
apiVersion: forge/v1
kind: Deployment
metadata: { name: s16-py, environment: dev }
spec:
  runtime: systemd
  artifact:
    archive:
      url: https://10.50.0.1:8443/acme/hello-py/releases/download/v1/hello-py-v1.tar.gz
      sha256: <from fixtures.py>
  template: { command: python3 app.py, listenPort: 8090 }
  placement: { nodes: [lab-b] }
  strategy: { type: recreate }
  health:
    checks:
      readiness: [{ name: ready, probe: { type: http, port: 8090, path: /ready } }]
```

## D09 · A .tar.gz release asset

**Expected:** exit 0; `curl http://127.0.0.1:8090/` on lab-b → `hello-py v1`;
the code is in `/srv/forge/apps/dev/s16-py/releases/1/`, owned by
`dev-s16-py`, with `.forge-release` holding `sha256 <hex>`; the process runs
as `dev-s16-py`, not root.

## D10 · A .zip, start-first on two nodes behind an endpoint

`environment: staging`, `nodes: [lab-b, lab-c]`, archive
`hello-py-v1.zip` with `stripComponents: 1`, recreate; endpoint
`s16.lab.local` on port 8090; then `v2` with
`{ type: rolling, replacement: start-first }` under a request loop.

**Expected:** no failed request; every answer `hello-py v2`; both nodes serve
from slot `a` (`forge-app-staging-s16-py--a.service`) on its own port, which
the service gets in `$PORT`.

## D11 · A pinned Git commit

`s16-git` with `git: { repo: https://10.50.0.1:8443/acme/hello-py.git,
commit: <v2 commit> }`.

**Expected:** exit 0; `releases/1/VERSION` is `v2`; `.forge-release` holds
`git <commit>`.

## Clean up

Remove the endpoint and services `s16-py` (dev, staging), `s16-git`,
`s16-job`.
