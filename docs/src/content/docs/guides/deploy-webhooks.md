---
title: Deploy from CI with a webhook
description: Create a secret deploy URL for a service or stack, sign deliveries, and call it from GitHub Actions, GitLab CI or curl.
sidebar:
  order: 9
---

This guide shows you how to let CI deploy a service or a compose stack by
calling a secret URL. It works for Default and Docker Swarm clusters.

## Before you begin

- You need `webhook.manage` on the workspace (MFA-guarded) and the right to
  deploy the target: `deploy` on the service, or `stack.manage` on the
  stack's environment.
- A delivery runs with **your** permissions, checked again every time. If you
  lose the right to deploy, the webhook stops working.

## Steps

1. Open the service (or stack) and select the **Webhooks** tab, then
   **Create webhook**.
2. Choose a **Signature**:
   - **GitHub**: GitHub signs each delivery (`X-Hub-Signature-256`).
   - **GitLab**: GitLab sends the secret in `X-Gitlab-Token`.
   - **Forge**: sign with an HMAC and a timestamp (best for any CI).
   - **None**: the URL alone. Use it only on a trusted network.
3. Optionally set a **Tag pattern** such as `v*`. A delivery can then deploy
   a matching image tag. Without a pattern, the tag cannot be changed.
4. Select **Create**. Copy the **URL** and the **Secret** now: they are shown
   only once.

:::caution
Treat the URL as a password. Anyone with it (and the secret, when set) can
deploy the target. If it leaks, select **Rotate**: the old URL stops working
at once.
:::

## Call it from CI

**GitHub Actions** (Forge signature, deploy the pushed tag):

```yaml
- name: Deploy
  env:
    URL: ${{ secrets.FORGE_WEBHOOK_URL }}
    SECRET: ${{ secrets.FORGE_WEBHOOK_SECRET }}
  run: |
    body="{\"tag\":\"${GITHUB_REF_NAME}\"}"
    ts=$(date +%s)
    sig=$(printf '%s.%s' "$ts" "$body" | openssl dgst -sha256 -hmac "$SECRET" -hex | sed 's/^.* //')
    curl -fsS -X POST "$URL" -H "X-Forge-Timestamp: $ts" -H "X-Forge-Signature: sha256=$sig" -d "$body"
```

**GitLab CI** (GitLab signature):

```yaml
deploy:
  script:
    - 'curl -fsS -X POST "$FORGE_WEBHOOK_URL?tag=$CI_COMMIT_TAG" -H "X-Gitlab-Token: $FORGE_WEBHOOK_SECRET"'
```

**curl** (no signature):

```sh
curl -fsS -X POST "https://<forge-host>/hooks/<token>?tag=v1.4.0"
```

The answer is `202` with the jobs started and a `status_url`. To wait in CI,
poll it until `status` is `succeeded` or `failed`:

```sh
curl -fsS "https://<forge-host>/hooks/<token>/jobs/<job-id>"
```

## Verify

- The webhook's **Deliveries** list shows the call as **accepted** with its
  job.
- The service's deployment history shows the deploy, made by you through the
  webhook.

## Troubleshooting

| Answer | Cause | Fix |
| --- | --- | --- |
| `401` | Signature missing or wrong, or the timestamp is more than 5 minutes off | Sign the exact body; sync the CI clock |
| `404` | Wrong or rotated URL | Use the current URL |
| `422` | A tag was sent but the webhook has no pattern, or the tag does not match | Set a tag pattern, or send a matching tag |
| `403` | The webhook is disabled, or its creator can no longer deploy | Enable it, or recreate it as someone who can |
| `429` | Too many calls | Slow down; at most 10 per minute per webhook |

## Next steps

- [Run a Docker Swarm cluster](/docs/guides/docker-swarm/)
- [REST API: deploy webhooks](/docs/reference/rest-api/#deploy-webhooks)
