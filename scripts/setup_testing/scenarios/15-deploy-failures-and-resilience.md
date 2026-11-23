# 15 · Failures and resilience

What happens when things go wrong: a release that never gets ready, an
artifact that does not match, a commit that does not exist, the control
plane restarting in the middle of a rollout, a node dropping out.

**Run it:** `python 15-deploy-failures-and-resilience.py [--only D16] [--keep]`

## Prerequisites

`10-deploy-setup.md`. `s15-web` (whoami, `staging`, lab-c) behind endpoint
`s15.lab.local`; the native cases use `dev` on lab-b.

## D07 · Never ready

Start-first with image `nginx:1.27-alpine`, entrypoint `/bin/sh -c exit`,
`startupTimeout: 45s`, under a request loop.

**Expected:** exit `10`; state `failed` (not `rolled_back`: no traffic moved);
error `… did not become ready and stable within 45s: …`; only the old copy is
left; no failed request.

## D12 · Checksum mismatch

`s15-py` (native service) with archive
`https://10.50.0.1:8443/acme/hello-py/releases/download/broken/hello-py-v2.tar.gz`
and the SHA-256 of the real file (the mock changes one byte).

**Expected:** exit `10`; error `the archive's SHA-256 is …, not …: refused`;
no `forge-app-dev-s15-py*` unit on lab-b.

## D11b · Missing commit

`s15-git` with `git: { repo: https://10.50.0.1:8443/acme/hello-py.git,
commit: 0000…0000 }`.

**Expected:** exit `10`; error starts with `git fetch 0000… failed`.

## D16 · The control plane restarts during a gate

Start a start-first rollout with `stabilizationPeriod: 60s` (no `--wait`),
then 20 s later restart the server:

```powershell
forge nodes exec lab-a --login root --shell -- "systemd-run --on-active=2s systemctl restart forge-server"
```

**Expected:** the deployment resumes the step it was on and succeeds; the
request loop through the edge sees no failure.

## D16 · The node drops out

```powershell
forge nodes exec lab-c --login root --shell -- "systemd-run --on-active=3s /bin/sh -c 'systemctl stop forge-agent; sleep 40; systemctl start forge-agent'"
forge deploy apply -f s15.deploy.yaml --wait
```

**Expected:** the rollout waits for the node to come back and completes
(exit 0).

## Clean up

Remove the endpoint and services `s15-web` (staging), `s15-py` and
`s15-git` (dev).
