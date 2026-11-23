---
title: Run from source
description: Build Forge from a source checkout, run a control plane and enroll an agent on one machine.
sidebar:
  order: 2
---


This page runs Forge from a source checkout on one machine, for people
working on Forge itself. To install a release, see the
[Quickstart](/docs/getting-started/quickstart/).

## 1. Build

```sh
cargo build --release -p forge-server -p forge-agent -p forge-plugin-runner -p forge-analytics-worker
(cd src/ui && npm ci && npm run build)
```

Binaries land in `target/release/`.

## 2. Configure and initialise

Create `forge.toml`:

```toml
data_dir = "./.forge-data"
api_listen = "127.0.0.1:8080"
agent_listen = "0.0.0.0:7443"
public_url = "http://localhost:8080"
agent_public_url = "https://localhost:7443"
cookie_secure = false        # local HTTP only; keep true behind TLS
ui_dir = "src/ui/dist"
```

Create the first platform administrator. The password comes from the
`FORGE_ADMIN_PASSWORD` environment variable (or stdin), never from
command-line arguments:

```sh
export FORGE_ADMIN_PASSWORD='choose a long passphrase'
forge-server --config forge.toml init --username admin
unset FORGE_ADMIN_PASSWORD
```

## 3. Start the control plane

```sh
forge-server --config forge.toml serve
```

Open `http://localhost:8080` in a browser and sign in.

## 4. Enroll an agent

In the console, open **Overview**, select **Add node**, then the
**Manual (one-line installer)** tab, and select **Generate token**. Or create
the token with an [API key](/docs/guides/settings-and-api-keys/):

```sh
curl -s -X POST http://localhost:8080/api/v1/enrollment-tokens \
  -H "Authorization: Bearer $FORGE_API_KEY" \
  -H 'Content-Type: application/json' -d '{}'
```

The response contains the token and ready-made enroll commands. On the
machine to manage (here the same machine, so the agent binary is in
`target/release/`):

```sh
printf 'Enrollment token: '; stty -echo; read -r FORGE_ENROLL_TOKEN; stty echo; echo; export FORGE_ENROLL_TOKEN
forge-agent --state-dir ./agent-state enroll --server https://localhost:7443
unset FORGE_ENROLL_TOKEN
forge-agent --state-dir ./agent-state run
```

Paste the token at the `Enrollment token:` prompt. It is not echoed or kept
in shell history.

The token embeds the SHA-256 fingerprint of the control plane's CA, so the
agent verifies it is talking to the right control plane before sending
anything.

## 5. Send a command

In the console, open the node and select **Ping**, or open the row menu of
the node on **Overview** and select **Ping**. The jobs drawer shows the
`agent.command` job moving through
`dispatch` → `await_result` to `succeeded`.

## Next steps

- [Add nodes](/docs/guides/add-nodes/) with the installer or an SSH login
- [Roles and access](/docs/guides/roles-and-access/)
- [Enable MFA](/docs/guides/mfa/): sensitive actions require it
- [Write a plugin](/docs/guides/writing-plugins/)
