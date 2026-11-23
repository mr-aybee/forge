---
title: Deploy from CI
description: Roll out a service from GitHub Actions or GitLab CI with a service account that may deploy only that service.
sidebar:
  order: 8
---

This guide shows you how to let a CI pipeline deploy one service with
`forge deploy apply --wait`, using a service account whose access is limited
to that service. For the document, see
[Deploy a service from YAML](/docs/guides/deploy-with-yaml/).

## Before you begin

- **Permissions.** To set up the account you need `access.manage` and
  `user.manage` on the service, with an MFA-verified session
  (`forge auth step-up`). You can only hand out actions you hold.
- **A service that has been deployed once**, so it exists and has its nodes
  and secrets. A first apply that creates the service needs `deploy` on the
  environment instead.
- **A `forge.deploy.yaml`** in the repository. See
  [Deployment YAML](/docs/reference/deployment-yaml/).
- **The CLI** on the runner: the static Linux build from the release assets.
  See [Install](/docs/reference/cli/#install).
- **Network.** The runner reaches the Forge server over HTTPS.

## Steps

1. Create an access profile with only what a deploy needs:

   ```sh
   forge access profiles create "CI deployer" --action view --action deploy --action job.create
   ```

2. Create the service account and give it the profile on the one service:

   ```sh
   forge service-accounts create ci-web --description "Deploys web from CI"
   forge services show web -n prod -o json | jq -r '.result.id'
   forge access assignments add svc-ci-web "CI deployer" service:<service-id> --yes
   ```

   The account can now deploy, roll back, pause and abort `web` only. It
   cannot approve: approvals need `deploy.approve` and a person.

3. Issue a token and store it as a CI secret:

   ```sh
   forge auth step-up
   (umask 077; forge -o json service-accounts token-create ci-web gha --expires-days 90 \
     --allow service:<service-id>=view,deploy,job.create \
     | jq -r '.result.secret' > ci-web.token)
   ```

   Save the file's content as the secret `FORGE_TOKEN` in your CI system,
   then delete the file. Save the server URL as `FORGE_SERVER`.

4. Add the pipeline. Pin the image by digest, or let Forge pin the tag on
   the first pull.

   **GitHub Actions**

   ```yaml
   # .github/workflows/deploy.yml
   name: deploy
   on:
     push:
       branches: [main]
   jobs:
     deploy:
       runs-on: ubuntu-latest
       steps:
         - uses: actions/checkout@v4
         - name: Install the Forge CLI
           run: |
             curl -fsSL -o forge "<forge-cli-download-url>/forge-cli-x86_64-unknown-linux-musl"
             chmod +x forge
         - name: Set the image
           run: |
             sed -i "s|image: .*|image: ghcr.io/<org>/web:${GITHUB_SHA}|" forge.deploy.yaml
         - name: Deploy
           env:
             FORGE_SERVER: ${{ secrets.FORGE_SERVER }}
             FORGE_TOKEN: ${{ secrets.FORGE_TOKEN }}
           run: ./forge --non-interactive deploy apply -f forge.deploy.yaml --environment prod --wait --timeout 30
   ```

   **GitLab CI**

   ```yaml
   # .gitlab-ci.yml
   deploy:
     stage: deploy
     image: alpine:3
     rules:
       - if: $CI_COMMIT_BRANCH == "main"
     script:
       - apk add --no-cache curl
       - curl -fsSL -o forge "<forge-cli-download-url>/forge-cli-x86_64-unknown-linux-musl"
       - chmod +x forge
       - sed -i "s|image: .*|image: registry.gitlab.com/<group>/web:${CI_COMMIT_SHA}|" forge.deploy.yaml
       - ./forge --non-interactive deploy apply -f forge.deploy.yaml --environment prod --wait --timeout 30
   ```

   In GitLab, set `FORGE_SERVER` and `FORGE_TOKEN` as masked, protected
   CI/CD variables.

:::caution
Pass the token only through the `FORGE_TOKEN` environment variable. Never
put it in the YAML, a command line or a URL. Rotate it before it expires,
and revoke it with `forge service-accounts token-revoke` when you stop
using it.
:::

The step's exit status tells the pipeline what happened:

| Exit | Meaning |
| --- | --- |
| `0` | The release is live |
| `10` | The rollout failed, was aborted or was rolled back |
| `8` | `--timeout` passed; the rollout goes on in Forge |
| `7` | The service already has a deployment running |
| `4` | The token may not deploy this service |
| `2` | The document or the plan was refused |

## Verify

- The pipeline step ends with `release <n> is live` and exit `0`.
- `forge deploy history web -n prod` shows the deployment, started by
  `svc-ci-web`.
- The audit log lists `deployment.apply` and `deployment.succeeded` for the
  service account.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| Exit `4`, permission denied | The profile is assigned at another scope, or lacks `job.create` | Assign the profile on `service:<service-id>` with `view`, `deploy` and `job.create` |
| Exit `4` on a new service | Creating a service needs `deploy` on the environment | Deploy the service once by hand, or assign the profile on `environment:<environment-id>` |
| Exit `5` | The document mounts host folders outside the app folder (`stack.privileged`, MFA) | Tokens never hold MFA; keep volumes inside the app folder |
| Exit `8` with a manual or approval strategy | The rollout waits for a person | Approve it in the console or with `forge deploy approve`, or raise `--timeout` |
| Exit `3` | `FORGE_TOKEN` or `FORGE_SERVER` is missing or the token expired | Check the CI variables; issue a new token |

## Next steps

- [Deploy a service from YAML](/docs/guides/deploy-with-yaml/)
- [`forge deploy`](/docs/reference/cli/deploy/)
- [`forge service-accounts`](/docs/reference/cli/service-accounts/)
- [Access profiles](/docs/guides/access-profiles/)
